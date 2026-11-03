//! `kin mcp`: read-only tools for Claude, over the pages the commands read.
//!
//! Every read goes through [`Reads`], which can only fetch a page and cannot send a
//! form, which is what every write at Kin takes. Tests put fixtures behind it, and
//! `tests/mcp_readonly.rs` puts a stub portal behind the real [`LivePortal`].
//!
//! The server reads the stored session and nothing else. It never signs in: a login
//! spends a one-time code, may read the mailbox, and repeated ones may lock the
//! account, so a lapsed session is an auth error naming `kin auth login`. `Portal`
//! would sign in again, and it writes the keychain, so it is not used here. The one
//! keychain item also holds the password; it is dropped unread. Kin rotates the
//! session cookie, so the server keeps the newest jar in memory for as long as the
//! keychain still holds the jar it came from, and a new sign-in takes effect at once.
//! A restart loses that copy and falls back to the stored one, which may have gone
//! stale: then `kin auth login` is needed even though the session was in use. A write
//! from a background server could block on a keychain dialog, so that is the trade
//! made here.
//!
//! The parsers are best-effort, for a person who can see the page. A tool's answer
//! goes to a model as fact, so a page that lost what the parsers look for is an
//! upstream error here, never an empty list, and a row they could not read is never
//! dropped in silence.
//!
//! A policy number becomes a path only after the portal's own list has named it, so
//! no argument can reach another customer's policy or any other page; `pick_policy`
//! in `main.rs` trusts a `KIN-` number and is not used here. Document links work
//! without the session, so no tool returns one, and the payments page's embedded
//! form is never read or posted.

use std::sync::{Mutex, MutexGuard, PoisonError};

use pk_cli_config::ConfigStore;
use pk_cli_core::{resolve, CliError};
use pk_cli_mcp::{
    parse_args, ListArgs, Page, PageRequest, Server, ToolDef, ToolError, ToolOutput, Toolset,
};
use pk_cli_scrape as scrape;
use pk_cli_secrets::CredentialStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bundle::Vault;
use crate::client::{Http, BASE};
use crate::config::Config;
use crate::dto;
use crate::jar::Jar;
use crate::parse::{self, Deductible, Line, PolicyDetail, PolicySummary, Term};
use crate::session::{self, page_for_term, parse_term, signed_in_body, TermSel, BIN};

const PORTAL: &str = "/kin_portal";

/// The tools `kin mcp` serves. Nothing here reads the config or the keychain: a
/// client lists tools before anyone has signed in.
pub fn tools() -> KinTools<LivePortal<CredentialStore>> {
    KinTools::new(LivePortal::new(
        BASE,
        CredentialStore::for_binary(BIN),
        ConfigStore::new(BIN),
    ))
}

/// The one way a tool reaches Kin: a signed-in page's HTML. It offers no way to send
/// a form.
pub trait Reads {
    fn page(&self, path: &str) -> Result<String, CliError>;
    /// The configured default policy, read on each call.
    fn default_policy(&self) -> Result<Option<String>, CliError>;
}

/// [`Reads`] over the portal, reading the config and the session on each call.
pub struct LivePortal<V> {
    base: String,
    vault: V,
    config: ConfigStore,
    rotated: Mutex<Rotated>,
}

/// The newest jar Kin issued, and the stored one it replaced. It is memory only,
/// since a tool does not write the keychain. A new sign-in changes the stored jar,
/// which retires this.
#[derive(Default)]
struct Rotated(Option<Held>);

struct Held {
    account: String,
    stored: Jar,
    newest: Jar,
}

impl Rotated {
    /// What to send, given what is stored now.
    fn current(&self, account: &str, stored: &Jar) -> Jar {
        match &self.0 {
            Some(held) if held.account == account && held.stored == *stored => held.newest.clone(),
            _ => stored.clone(),
        }
    }

    /// Remember the jar a good read left behind. A slower read can finish after a
    /// newer jar arrived, and must not put its older one back.
    ///
    /// An auth failure records nothing: Kin sets a fresh cookie even on the anonymous
    /// redirect to sign-in, and that must not replace a session that may still work.
    fn record(&mut self, account: &str, stored: &Jar, sent: &Jar, newest: Jar) {
        if self.current(account, stored) == *sent {
            self.0 = Some(Held {
                account: account.to_string(),
                stored: stored.clone(),
                newest,
            });
        }
    }
}

impl<V> LivePortal<V> {
    pub fn new(base: &str, vault: V, config: ConfigStore) -> Self {
        Self {
            base: base.to_string(),
            vault,
            config,
            rotated: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Rotated> {
        self.rotated.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn settings(&self) -> Result<Config, CliError> {
        // No tool argument can fix a bad config file, and the error names a path
        // under the home directory, which the user's terminal can show.
        self.config.load().map_err(|_| {
            CliError::Other(format!(
                "the {BIN} settings could not be read: ask the user to run `{BIN} config show` \
                 in a terminal"
            ))
        })
    }
}

impl<V: Vault> Reads for LivePortal<V> {
    fn page(&self, path: &str) -> Result<String, CliError> {
        let cfg = self.settings()?;
        let account = session::account(&cfg, None)?;
        // The keychain may wait on an approval dialog, and Kin on the network, so the
        // lock is held for neither.
        let stored = self
            .vault
            .load(&account)?
            .and_then(|bundle| bundle.jar)
            .filter(Jar::has_session)
            .ok_or_else(|| CliError::Auth("no Kin session is stored".into()))?;
        let send = self.lock().current(&account, &stored);

        let mut http = Http::at(&self.base, send.clone(), false)?;
        let page = http.get_following(path)?;
        if page.needs_login() {
            return Err(CliError::Auth("the Kin session has lapsed".into()));
        }
        let body = signed_in_body(path, page)?;
        self.lock().record(&account, &stored, &send, http.jar);
        Ok(body)
    }

    fn default_policy(&self) -> Result<Option<String>, CliError> {
        Ok(self.settings()?.policy)
    }
}

fn shape_error(what: &str) -> ToolError {
    ToolError::upstream(format!(
        "{what} did not read as expected, so Kin's page may have changed; nothing is reported \
         rather than a wrong answer"
    ))
    .with_fix("Tell the user this `kin` may need an update")
}

fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// `$1,234` as `1234.00`. Anything else (`Applied`, `Full`) is `None`.
fn dollars(text: &str) -> Option<String> {
    dto::money(text)?
        .get("amount")?
        .as_str()
        .map(str::to_string)
}

/// A number the portal listed, as long as it is safe to put in a path.
fn path_safe(number: &str) -> bool {
    !number.is_empty()
        && number
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `kin_policies`' arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct PoliciesArgs {
    #[serde(flatten)]
    list: ListArgs,
}

/// One policy on the login.
#[derive(Debug, Serialize, JsonSchema)]
struct PolicyItem {
    number: String,
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flood_endorsement: Option<bool>,
}

/// `kin_policy`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct PolicyArgs {
    /// Policy number, or part of its address. Default: the only one.
    #[serde(default)]
    policy: Option<String>,
    /// `current` (default), `previous`, or a term's start date, YYYY-MM-DD.
    #[serde(default)]
    term: Option<String>,
    /// `detailed` adds features, discounts, applicants and the lender's details.
    #[serde(default)]
    response_format: Option<pk_cli_mcp::ResponseFormat>,
}

/// A policy term.
#[derive(Debug, Serialize, JsonSchema)]
struct TermItem {
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<String>,
}

impl From<&Term> for TermItem {
    fn from(t: &Term) -> Self {
        TermItem {
            label: t.label.clone(),
            start: t.start.clone(),
            end: t.end.clone(),
        }
    }
}

/// A deductible, coverage or feature: `amount` when it is money, else `value`.
#[derive(Debug, Serialize, JsonSchema)]
struct LineItem {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    basis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

impl LineItem {
    fn of(name: &str, text: &str) -> Self {
        let amount = dollars(text);
        LineItem {
            name: name.to_string(),
            value: amount.is_none().then(|| text.to_string()),
            amount,
            basis: None,
            description: None,
        }
    }

    fn deductible(d: &Deductible, detailed: bool) -> Self {
        LineItem {
            basis: d.basis.clone(),
            description: d.detail.clone().filter(|_| detailed),
            ..LineItem::of(&d.name, &d.value)
        }
    }

    fn line(l: &Line, detailed: bool) -> Self {
        LineItem {
            description: l.description.clone().filter(|_| detailed),
            ..LineItem::of(&l.name, &l.value)
        }
    }
}

/// A labelled value from the page.
#[derive(Debug, Serialize, JsonSchema)]
struct Field {
    label: String,
    value: String,
}

/// `kin_policy`'s result.
#[derive(Debug, Serialize, JsonSchema)]
struct PolicyOut {
    number: String,
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    flood_endorsement: bool,
    /// The term the limits below belong to.
    #[serde(skip_serializing_if = "Option::is_none")]
    term: Option<TermItem>,
    /// Today's premium, whichever term is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    current_premium: Option<String>,
    /// Today's policy period, whichever term is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    current_period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flood_premium: Option<String>,
    deductibles: Vec<LineItem>,
    coverages: Vec<LineItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discount_total: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lender: Option<String>,
    terms: Vec<TermItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<Vec<LineItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discounts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    applicants: Option<Vec<Field>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loan_number: Option<String>,
    /// The lender's other fields, and the page's other labelled items.
    #[serde(skip_serializing_if = "Option::is_none")]
    other: Option<Vec<Field>>,
}

impl PolicyOut {
    fn of(d: PolicyDetail, term: Option<&Term>, detailed: bool) -> Self {
        let mut current_premium = None;
        let mut current_period = None;
        let mut applicants = Vec::new();
        let mut other = Vec::new();
        for (label, value) in &d.header {
            match label.as_str() {
                "Current premium" => current_premium = dollars(value),
                "Policy period" => current_period = Some(value.clone()),
                l if l.to_ascii_lowercase().contains("applicant") => applicants.push(Field {
                    label: label.clone(),
                    value: value.clone(),
                }),
                _ => other.push(Field {
                    label: label.clone(),
                    value: value.clone(),
                }),
            }
        }
        // Kin lists the flood endorsement's premium in the deductibles card; it is a
        // premium for the shown term, not a deductible.
        let (premiums, deductibles): (Vec<_>, Vec<_>) =
            d.deductibles.iter().partition(|x| x.key == "flood-premium");
        if let Some(lender) = d.lender.as_ref().filter(|_| detailed) {
            other.extend(lender.fields.iter().map(|(label, value)| Field {
                label: label.clone(),
                value: value.clone(),
            }));
        }
        PolicyOut {
            flood_premium: premiums.first().and_then(|p| dollars(&p.value)),
            deductibles: deductibles
                .into_iter()
                .map(|x| LineItem::deductible(x, detailed))
                .collect(),
            coverages: d
                .coverages
                .iter()
                .map(|l| LineItem::line(l, detailed))
                .collect(),
            discount_total: d.discount_total.as_deref().and_then(dollars),
            lender: d.lender.as_ref().map(|l| l.name.clone()),
            terms: d.terms.iter().map(TermItem::from).collect(),
            term: term.map(TermItem::from),
            current_premium,
            current_period,
            features: detailed
                .then(|| d.features.iter().map(|l| LineItem::line(l, true)).collect()),
            discounts: detailed.then(|| d.discounts.clone()),
            applicants: detailed.then_some(applicants),
            loan_number: d
                .lender
                .as_ref()
                .filter(|_| detailed)
                .and_then(|l| l.loan_number.clone()),
            other: detailed.then_some(other),
            number: d.number,
            address: d.address,
            status: d.status,
            kind: d.kind,
            flood_endorsement: d.flood,
        }
    }
}

/// `kin_documents`' arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct DocumentArgs {
    /// Policy number, or part of its address. Default: the only one.
    #[serde(default)]
    policy: Option<String>,
    /// `current` (default), `previous`, or a term's start date, YYYY-MM-DD.
    #[serde(default)]
    term: Option<String>,
    #[serde(flatten)]
    list: ListArgs,
}

/// One document, without its link.
#[derive(Debug, Serialize, JsonSchema)]
struct DocumentItem {
    /// What `kin documents download` takes.
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
}

/// `kin_documents`' result.
#[derive(Debug, Serialize, JsonSchema)]
struct DocumentsOut {
    policy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    term: Option<TermItem>,
    #[serde(flatten)]
    page: Page<DocumentItem>,
}

/// `kin_payments`' arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct PaymentArgs {
    /// Policy number, or part of its address. Default: the only one.
    #[serde(default)]
    policy: Option<String>,
    /// Past (paid, refunded) payments instead of upcoming ones.
    #[serde(default)]
    past: bool,
    /// Upcoming and past payments.
    #[serde(default)]
    all: bool,
    #[serde(flatten)]
    list: ListArgs,
}

/// One payment.
#[derive(Debug, Serialize, JsonSchema)]
struct PaymentItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    description: String,
    amount: String,
    /// `scheduled`, `paid`, `issued`.
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    card_last4: Option<String>,
}

/// `kin_payments`' result.
#[derive(Debug, Serialize, JsonSchema)]
struct PaymentsOut {
    policy: String,
    #[serde(flatten)]
    page: Page<PaymentItem>,
}

/// The toolset `kin mcp` serves.
pub struct KinTools<R> {
    server: Server,
    reads: R,
}

impl<R: Reads> KinTools<R> {
    pub fn new(reads: R) -> Self {
        let server = Server::new(BIN, env!("CARGO_PKG_VERSION"), "Kin Insurance", "kin")
            .instructions(
                "Access here is read-only: no tool can pay a bill, change a policy or its \
                 coverage, file a claim or change anything at Kin. Signing in spends a one-time \
                 code, so ask for it only when a tool says it is needed. Amounts are US dollars, \
                 as decimal strings. Document results name documents and carry no contents or \
                 links; the user saves one with `kin documents download <id> -o <path>`. \
                 kin_payments reports what Kin's payments page lists, which has shown no paging \
                 so far; if Kin cuts a long history short, nothing here would say so.",
            );
        Self { server, reads }
    }

    /// The login's policies. No customer has been seen with none, so an empty list is
    /// a page that changed, and a card that cannot be read would drop a policy.
    fn listed(&self) -> Result<Vec<PolicySummary>, ToolError> {
        let html = self.reads.page(PORTAL)?;
        let cards = scrape::blocks_with_class(&html, "div", "policy-summary").len();
        let all = parse::policies(&html);
        if all.is_empty() || all.len() != cards || !all.iter().all(|p| path_safe(&p.number)) {
            return Err(shape_error("the policy list"));
        }
        Ok(all)
    }

    /// The policy a call means, always one the portal itself listed.
    fn chosen(&self, policy: Option<&str>) -> Result<String, ToolError> {
        let all = self.listed()?;
        let wanted = match trimmed(policy) {
            Some(query) => Some(query.to_string()),
            None => self.reads.default_policy()?,
        };
        if let Some(query) = wanted {
            return resolve::pick(
                &all,
                &query,
                |p| vec![p.number.clone()],
                |p| &p.address,
                "policy",
            )
            .map(|p| p.number.clone())
            .map_err(|e| {
                ToolError::from(e).with_fix("Call kin_policies to see them, then pass policy")
            });
        }
        let active: Vec<_> = all
            .iter()
            .filter(|p| p.status.as_deref() == Some("Active"))
            .collect();
        match (all.len(), active.len()) {
            (1, _) => Ok(all[0].number.clone()),
            (_, 1) => Ok(active[0].number.clone()),
            _ => Err(ToolError::usage(format!(
                "this login has {} policies and no default: {}",
                all.len(),
                all.iter()
                    .map(|p| p.number.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .with_fix("Pass policy")),
        }
    }

    /// A page for the term asked for. Kin ignoring `?term=` would show the current
    /// term's figures as another's, so the page must say it shows that term.
    fn at_term(&self, path: &str, want: &TermSel) -> Result<(String, Option<Term>), ToolError> {
        let (html, term) = page_for_term(|p| self.reads.page(p), path, want)?;
        if *want != TermSel::Current {
            let shows = parse::terms(&html).into_iter().find(|t| t.selected);
            if shows.map(|t| t.value) != term.as_ref().map(|t| t.value.clone()) {
                return Err(shape_error("the page for that term"));
            }
        }
        Ok((html, term))
    }

    fn policies(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: PoliciesArgs = parse_args(args)?;
        let page = PageRequest::new(&args.list, &())?;
        let detailed = args.list.format().is_detailed();
        let items = self
            .listed()?
            .into_iter()
            .map(|p| PolicyItem {
                number: p.number,
                address: p.address,
                status: p.status,
                kind: p.kind.filter(|_| detailed),
                flood_endorsement: detailed.then_some(p.flood),
            })
            .collect();
        ToolOutput::new(&page.page(items, "limit")?)
    }

    fn policy(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: PolicyArgs = parse_args(args)?;
        let want = term_of(args.term.as_deref())?;
        let detailed = args.response_format.unwrap_or_default().is_detailed();
        let number = self.chosen(args.policy.as_deref())?;
        let path = format!("/kin_portal/policies/{number}/details");
        let (html, term) = self.at_term(&path, &want)?;
        let detail = parse::policy_detail(&html)
            .filter(|d| d.number == number)
            .filter(|d| !d.coverages.is_empty() || !d.deductibles.is_empty())
            .ok_or_else(|| shape_error("the policy's details"))?;
        ToolOutput::new(&PolicyOut::of(detail, term.as_ref(), detailed))
    }

    fn documents(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: DocumentArgs = parse_args(args)?;
        let want = term_of(args.term.as_deref())?;
        let policy = trimmed(args.policy.as_deref());
        let page = PageRequest::new(&args.list, &(policy, args.term.as_deref()))?;
        let detailed = args.list.format().is_detailed();
        let number = self.chosen(policy)?;
        let path = format!("/kin_portal/policies/{number}/documents");
        let (html, term) = self.at_term(&path, &want)?;
        // Every policy has a declarations page, so an empty list is a changed page.
        let rows = scrape::blocks_with_class(&html, "div", "policy-document__item").len();
        let documents = parse::documents(&html);
        if documents.is_empty() || documents.len() != rows {
            return Err(shape_error("the policy's documents"));
        }
        let items = documents
            .into_iter()
            .map(|d| DocumentItem {
                id: d.label,
                name: d.name,
                description: d.description.filter(|_| detailed),
                file: d.file.filter(|_| detailed),
            })
            .collect();
        ToolOutput::new(&DocumentsOut {
            policy: number,
            term: term.as_ref().map(TermItem::from),
            page: page.page(items, "policy or term")?,
        })
    }

    fn payments(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: PaymentArgs = parse_args(args)?;
        if args.past && args.all {
            return Err(ToolError::usage("pass past or all, not both"));
        }
        let policy = trimmed(args.policy.as_deref());
        let page = PageRequest::new(&args.list, &(policy, args.past, args.all))?;
        let detailed = args.list.format().is_detailed();
        let number = self.chosen(policy)?;
        let base = format!("/kin_portal/policies/{number}/payments");
        let mut views = Vec::new();
        if !args.past {
            views.push("upcoming");
        }
        if args.past || args.all {
            views.push("approved");
        }
        let mut items = Vec::new();
        for view in views {
            let html = self.reads.page(&format!("{base}?billing_status={view}"))?;
            items.extend(payments_on(&html, detailed)?);
        }
        ToolOutput::new(&PaymentsOut {
            policy: number,
            page: page.page(items, "policy, past or all")?,
        })
    }
}

fn term_of(raw: Option<&str>) -> Result<TermSel, ToolError> {
    match trimmed(raw) {
        Some(raw) => parse_term(raw).map_err(ToolError::usage),
        None => Ok(TermSel::Current),
    }
}

/// The rows of a payments page. Nobody owing anything is a real answer, so the page
/// must still show the list's frame, and an amount in it with no row read means the
/// rows changed. A row with no amount is never a payment of nothing.
fn payments_on(html: &str, detailed: bool) -> Result<Vec<PaymentItem>, ToolError> {
    let what = "the policy's payments";
    let frame = scrape::block_by_id(html, "turbo-frame", "billing-transactions")
        .ok_or_else(|| shape_error(what))?;
    let rows = scrape::blocks_with_class(&frame, "div", "billing-transaction__item").len();
    let read = parse::payments(&frame);
    if read.len() != rows || (read.is_empty() && frame.contains('$')) {
        return Err(shape_error(what));
    }
    read.into_iter()
        .map(|p| {
            Ok(PaymentItem {
                amount: dollars(&p.amount).ok_or_else(|| shape_error(what))?,
                date: p.date,
                description: p.description,
                status: p.status,
                card_last4: p.card_last4.filter(|_| detailed),
            })
        })
        .collect()
}

impl<R: Reads> Toolset for KinTools<R> {
    fn server(&self) -> Server {
        self.server.clone()
    }

    fn tools(&self) -> Vec<ToolDef> {
        let mut tools = vec![
            ToolDef::read(
                "kin_policies",
                "Kin policies",
                "Lists the policies on the user's Kin login. Use it to find a policy's number or \
                 see which are active. Items carry number, address and status (detailed adds \
                 kind and flood_endorsement).",
            )
            .input::<PoliciesArgs>()
            .output::<Page<PolicyItem>>(),
            ToolDef::read(
                "kin_policy",
                "Kin policy coverage",
                "Shows one Kin policy's coverage for a term. Use it for what is covered, the \
                 limits, deductibles and premium. Returns number, address, status, the term \
                 shown, current_premium, current_period, flood_premium, deductibles, coverages, \
                 discount_total, lender and terms (detailed adds features, discounts, \
                 applicants, loan_number and other).",
            )
            .input::<PolicyArgs>()
            .output::<PolicyOut>(),
            ToolDef::read(
                "kin_documents",
                "Kin policy documents",
                "Lists the documents on a Kin policy for a term, such as the declarations page. \
                 Use it to see whether a document exists. Returns policy, term and items with id \
                 and name (detailed adds description and file). It gives no contents or links.",
            )
            .input::<DocumentArgs>()
            .output::<DocumentsOut>(),
            ToolDef::read(
                "kin_payments",
                "Kin payments",
                "Lists a Kin policy's payments: upcoming ones by default, past ones with past, \
                 both with all. Use it for what is due and what was paid. Returns policy and \
                 items with date, description, amount and status (detailed adds card_last4).",
            )
            .input::<PaymentArgs>()
            .output::<PaymentsOut>(),
        ];
        tools.iter_mut().for_each(tighten);
        tools
    }

    async fn call(&self, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
        match name {
            "kin_policies" => self.policies(args),
            "kin_policy" => self.policy(args),
            "kin_documents" => self.documents(args),
            "kin_payments" => self.payments(args),
            _ => Err(ToolError::other(format!("unknown tool `{name}`"))),
        }
    }
}

/// The shared argument descriptions take a share of the 8,000-character budget the
/// definitions ride along in every prompt under.
fn tighten(tool: &mut ToolDef) {
    for (arg, text) in [
        (
            "cursor",
            "A result's next_cursor, to continue the same query.",
        ),
        ("response_format", "`detailed` adds the fuller fields."),
    ] {
        if let Some(description) = tool
            .input_schema
            .pointer_mut(&format!("/properties/{arg}/description"))
        {
            *description = Value::String(text.into());
        }
    }
}

#[cfg(test)]
mod tests;
