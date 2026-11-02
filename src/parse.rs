//! Reading the portal's server-rendered pages.
//!
//! `app.kin.com` publishes no JSON API: every page is Rails-rendered HTML.
//! The parsers key on the hooks Kin's own test suite uses (`data-rspec`,
//! `data-testid`, `data-document-label`) and on BEM class names
//! (`policy-summary`, `billing-transaction__item`), never on layout classes,
//! which change with every restyle. `docs/api.md` records each page and hook.
//!
//! Every function is total: markup it does not recognize yields `None` or an
//! empty list, so a redesign shows up as an empty result the caller turns
//! into an exit-5 "page changed" error, never a panic.

use pk_cli_core::dates;
use pk_cli_scrape as scrape;

// ---------------------------------------------------------------- helpers

/// Collapse whitespace.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text of the first `<name>` element whose class list holds `class`.
fn text_with_class(html: &str, name: &str, class: &str) -> Option<String> {
    scrape::elements(html, name)
        .into_iter()
        .find(|e| has_class(&e.tag, class))
        .map(|e| squash(&e.text))
}

fn has_class(tag: &str, class: &str) -> bool {
    scrape::attr(tag, "class").is_some_and(|c| c.split_whitespace().any(|w| w == class))
}

/// Every element carrying a `data-rspec`, in document order, as
/// `(rspec, text)`. Kin tags each coverage label, value and deductible this
/// way for its own tests, which makes it the most stable hook on the page.
fn rspec_elements(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let needle = "data-rspec=\"";
    let mut from = 0;
    while let Some(rel) = html[from..].find(needle) {
        let at = from + rel;
        from = at + needle.len();
        let Some(tag_start) = html[..at].rfind('<') else {
            continue;
        };
        let name: String = html[tag_start + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if name.is_empty() {
            continue;
        }
        let Some(el) = scrape::elements(&html[tag_start..], &name)
            .into_iter()
            .next()
        else {
            continue;
        };
        if let Some(key) = scrape::attr(&el.tag, "data-rspec") {
            out.push((key, squash(&el.text)));
        }
    }
    out
}

/// The markup between two markers, for scoping a search to one section.
fn between<'a>(html: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = html.find(start)?;
    let rest = &html[i..];
    let j = rest.find(end).unwrap_or(rest.len());
    Some(&rest[..j])
}

/// Percent-decode a URL component (`p......%40example.com`).
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A query parameter's decoded value.
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| percent_decode(v))
    })
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// `March 5, 2026` or `Nov 30, 2026` → `2026-03-05`.
pub fn parse_month_date(s: &str) -> Option<String> {
    let cleaned = s.replace(',', " ");
    let mut parts = cleaned.split_whitespace();
    let month = parts.next()?.to_ascii_lowercase();
    let m = MONTHS.iter().position(|p| month.starts_with(p))? as u32 + 1;
    let d: u32 = parts.next()?.parse().ok()?;
    let y: i64 = parts.next()?.parse().ok()?;
    if !(1..=31).contains(&d) || !(1900..=2200).contains(&y) {
        return None;
    }
    Some(dates::fmt_iso((y, m, d)))
}

/// `03/05/2026` → `2026-03-05`.
pub fn parse_slash_date(s: &str) -> Option<String> {
    dates::parse_mm_slash_dd_yyyy(s.trim()).map(dates::fmt_iso)
}

/// The two `MM/DD/YYYY` dates of a term label, as ISO.
fn term_dates(label: &str) -> (Option<String>, Option<String>) {
    let found: Vec<String> = label
        .split(|c: char| !(c.is_ascii_digit() || c == '/'))
        .filter(|w| w.len() == 10)
        .filter_map(parse_slash_date)
        .collect();
    (found.first().cloned(), found.get(1).cloned())
}

// ------------------------------------------------------------- login flow

/// The CSRF token of the form posting to `action`.
pub fn form_token(html: &str, action: &str) -> Option<String> {
    let marker = format!("action=\"{action}\"");
    let at = html.find(&marker)?;
    let form_start = html[..at].rfind("<form")?;
    let form = &html[form_start..];
    let form = &form[..form.find("</form>").unwrap_or(form.len())];
    scrape::elements(form, "input")
        .into_iter()
        .chain(bare_inputs(form))
        .find(|e| scrape::attr(&e.tag, "name").as_deref() == Some("authenticity_token"))
        .and_then(|e| scrape::attr(&e.tag, "value"))
}

/// `<input>` is a void element: `scrape::elements` only finds it when it is
/// (wrongly) closed, so read the bare opening tags too.
fn bare_inputs(html: &str) -> Vec<scrape::Element> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = html[from..].find("<input") {
        let at = from + rel;
        let end = html[at..]
            .find('>')
            .map(|e| at + e + 1)
            .unwrap_or(html.len());
        out.push(scrape::Element {
            tag: html[at..end].to_string(),
            text: String::new(),
        });
        from = end;
    }
    out
}

/// A delivery channel the code step offers: `text` or `email`, with the
/// masked destination Kin shows (`p......@example.com`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtpChannel {
    pub method: String,
    pub label: String,
}

/// The channels on the "Where should we send the code?" page.
pub fn otp_channels(html: &str) -> Vec<OtpChannel> {
    let labels: Vec<String> = scrape::elements(html, "label")
        .into_iter()
        .map(|e| squash(&e.text))
        .filter(|t| t.to_ascii_lowercase().contains("code to"))
        .collect();
    bare_inputs(html)
        .into_iter()
        .filter(|e| scrape::attr(&e.tag, "name").as_deref() == Some("otp_method"))
        .filter_map(|e| scrape::attr(&e.tag, "value"))
        .map(|method| {
            let key = if method == "text" { "text" } else { "email" };
            let label = labels
                .iter()
                .find(|l| l.to_ascii_lowercase().starts_with(key))
                .cloned()
                .unwrap_or_default();
            OtpChannel { method, label }
        })
        .collect()
}

/// A page only a signed-in session sees: it carries the sign-out form.
pub fn is_signed_in(html: &str) -> bool {
    html.contains("action=\"/users/sign_out\"")
}

/// The sign-in page, which a lapsed session is redirected to.
pub fn is_login_page(html: &str) -> bool {
    html.contains("name=\"user[password]\"")
}

/// The flash or inline error Kin shows on a re-rendered form, if any.
pub fn form_error(html: &str) -> Option<String> {
    for class in ["flash__message", "alert", "input__error", "flash"] {
        for name in ["div", "p", "span"] {
            if let Some(t) = text_with_class(html, name, class) {
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
    }
    None
}

// --------------------------------------------------------------- policies

/// One policy card on the portal's home page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySummary {
    pub number: String,
    pub address: String,
    pub status: Option<String>,
    /// `Homeowner`, from "Homeowner policy number:".
    pub kind: Option<String>,
    /// The card carries Kin's "Flood Coverage" badge.
    pub flood: bool,
}

/// The number and kind out of a `… policy number: <span>NUMBER</span>` block.
fn number_and_kind(block: &str, class: &str) -> Option<(String, Option<String>)> {
    let p = scrape::elements(block, "p")
        .into_iter()
        .find(|e| has_class(&e.tag, class))?;
    let text = squash(&p.text);
    let (label, number) = text.split_once("policy number:")?;
    let number = number.trim().to_string();
    if number.is_empty() {
        return None;
    }
    let kind = Some(label.trim().to_string()).filter(|k| !k.is_empty());
    Some((number, kind))
}

fn pill(block: &str) -> Option<String> {
    scrape::elements(block, "kin-pill")
        .into_iter()
        .next()
        .map(|e| squash(&e.text))
        .filter(|s| !s.is_empty())
}

/// The policies on `/kin_portal`.
pub fn policies(html: &str) -> Vec<PolicySummary> {
    scrape::blocks_with_class(html, "div", "policy-summary")
        .into_iter()
        .filter_map(|card| {
            let (number, kind) = number_and_kind(&card, "policy-summary__policy-number")?;
            Some(PolicySummary {
                number,
                address: text_with_class(&card, "span", "policy-summary__heading-text")
                    .unwrap_or_default(),
                status: pill(&card),
                kind,
                flood: card.contains("policy-summary__flood_endorsement"),
            })
        })
        .collect()
}

/// One option of a page's `term` selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// The selector's value, sent back as `?term=`.
    pub value: String,
    pub label: String,
    pub selected: bool,
    pub start: Option<String>,
    pub end: Option<String>,
}

/// The `term` selector of a details or documents page.
pub fn terms(html: &str) -> Vec<Term> {
    let Some(select) = scrape::block_by_id(html, "select", "term") else {
        return Vec::new();
    };
    let mut terms: Vec<Term> = scrape::elements(&select, "option")
        .into_iter()
        .filter_map(|o| {
            let value = scrape::attr(&o.tag, "value")?;
            let label = squash(&o.text);
            let (start, end) = term_dates(&label);
            Some(Term {
                value,
                selected: o.tag.contains(" selected"),
                label,
                start,
                end,
            })
        })
        .collect();
    // The documents page labels the current term just "Current Term". Terms
    // are listed newest first and run back to back, so an undated term
    // starts where the one after it ends.
    for i in 0..terms.len().saturating_sub(1) {
        if terms[i].start.is_none() {
            terms[i].start = terms[i + 1].end.clone();
        }
    }
    terms
}

/// A deductible line, or the flood endorsement's premium, which Kin lists
/// in the same card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deductible {
    /// Kin's hook: `standard-deductible`, `hurricane-deductible`,
    /// `flood-premium`.
    pub key: String,
    pub name: String,
    /// The text after the name ("Applies to most claims, …").
    pub detail: Option<String>,
    /// `(2% of Coverage A)` on a percentage deductible.
    pub basis: Option<String>,
    pub value: String,
}

/// One coverage or feature line: a label and its value (`$500,000`,
/// `Applied`, `Not applied`, `Full`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Kin's hook, e.g. `coverage_a`, `flood_and_water_backup`.
    pub key: String,
    pub name: String,
    pub value: String,
    pub description: Option<String>,
}

/// The mortgagee block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lender {
    pub name: String,
    pub loan_number: Option<String>,
    /// The other labelled fields (`Mortgagee clause`, `Fax number`, `Email`).
    pub fields: Vec<(String, String)>,
}

/// Everything on `/kin_portal/policies/<number>/details`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicyDetail {
    pub number: String,
    pub address: String,
    pub status: Option<String>,
    pub kind: Option<String>,
    pub flood: bool,
    /// The labelled header items, in page order (`Current premium`,
    /// `Policy period`, `Primary applicant`, `Co-applicant`).
    pub header: Vec<(String, String)>,
    pub terms: Vec<Term>,
    pub deductibles: Vec<Deductible>,
    pub coverages: Vec<Line>,
    pub features: Vec<Line>,
    pub discount_total: Option<String>,
    pub discounts: Vec<String>,
    pub lender: Option<Lender>,
}

/// The deductibles card: from its heading to the primary coverages.
fn deductibles(html: &str) -> Vec<Deductible> {
    let Some(card) = between(html, "id=\"deductibles\"", "id=\"primary-coverages\"") else {
        return Vec::new();
    };
    scrape::blocks_with_class(card, "div", "middle-sm")
        .into_iter()
        .filter_map(|row| {
            let ps = scrape::elements(&row, "p");
            let value_p = ps
                .iter()
                .find(|p| scrape::attr(&p.tag, "data-rspec").is_some())?;
            let key = scrape::attr(&value_p.tag, "data-rspec")?;
            let name = scrape::elements(&row, "span")
                .into_iter()
                .find(|s| has_class(&s.tag, "fw-bold"))
                .map(|s| squash(&s.text))?;
            let basis = scrape::elements(&row, "span")
                .into_iter()
                .find(|s| {
                    scrape::attr(&s.tag, "data-rspec").is_some_and(|k| k.ends_with("-percentage"))
                })
                .map(|s| squash(&s.text));
            let described = ps
                .iter()
                .find(|p| scrape::attr(&p.tag, "data-rspec").is_none())
                .map(|p| squash(&p.text))
                .unwrap_or_default();
            let mut detail = described.trim_start_matches(name.as_str()).to_string();
            if let Some(b) = &basis {
                detail = detail.replacen(b.as_str(), "", 1);
            }
            let detail = squash(&detail);
            Some(Deductible {
                key,
                name,
                detail: Some(detail).filter(|d| !d.is_empty()),
                basis,
                value: squash(&value_p.text),
            })
        })
        .collect()
}

/// Coverage and feature lines: each `<key>-label` paired with its
/// `<key>-value-1`, in page order. `coverage_*` keys are the primary limits;
/// the rest are the "other policy features".
fn lines(html: &str) -> (Vec<Line>, Vec<Line>) {
    let tagged = rspec_elements(html);
    let lookup = |key: &str| {
        tagged
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let mut coverages = Vec::new();
    let mut features = Vec::new();
    for (k, name) in &tagged {
        let Some(key) = k.strip_suffix("-label") else {
            continue;
        };
        let Some(value) = lookup(&format!("{key}-value-1")) else {
            continue;
        };
        let line = Line {
            key: key.to_string(),
            name: name.clone(),
            value,
            description: lookup(&format!("{key}-help-text-content-1")).filter(|d| !d.is_empty()),
        };
        if key.starts_with("coverage_") {
            coverages.push(line);
        } else {
            features.push(line);
        }
    }
    (coverages, features)
}

fn lender(html: &str) -> Option<Lender> {
    let block = between(html, "Your lender details", "</turbo-frame>")?;
    // `<br>` separates address lines; keep them apart once tags are stripped.
    let block = block.replace("<br>", ", ").replace("<br/>", ", ");
    let mut lender = Lender::default();
    let mut label: Option<String> = None;
    // Walk h3/p in document order: the first h3 is the lender's name, then
    // each h3 labels the p after it.
    let mut i = 0;
    while i < block.len() {
        let next_h3 = block[i..].find("<h3").map(|p| i + p);
        let next_p = block[i..].find("<p").map(|p| i + p);
        let (at, tag) = match (next_h3, next_p) {
            (Some(h), Some(p)) if h < p => (h, "h3"),
            (Some(h), None) => (h, "h3"),
            (_, Some(p)) => (p, "p"),
            (None, None) => break,
        };
        let Some(el) = scrape::elements(&block[at..], tag).into_iter().next() else {
            break;
        };
        let text = squash(&el.text);
        let end = block[at..]
            .find(&format!("</{tag}>"))
            .map(|e| at + e + tag.len() + 3)
            .unwrap_or(block.len());
        i = end;
        if text.is_empty() {
            continue;
        }
        match tag {
            "h3" if lender.name.is_empty() => lender.name = text,
            "h3" => label = Some(text),
            _ => {
                if let Some(n) = text.strip_prefix("Loan number:") {
                    lender.loan_number = Some(n.trim().to_string());
                } else if let Some(l) = label.take() {
                    lender.fields.push((l, text));
                }
            }
        }
    }
    (!lender.name.is_empty()).then_some(lender)
}

/// The details page. `None` when it carries no policy number (not a details
/// page, or a redesign).
pub fn policy_detail(html: &str) -> Option<PolicyDetail> {
    let (number, kind) = number_and_kind(html, "policy-detail__policy-number")?;
    let header = scrape::blocks_with_class(html, "div", "policy-detail__item")
        .into_iter()
        .filter_map(|item| {
            let text = scrape::strip_tags(&item);
            let (label, value) = text.split_once(':')?;
            Some((squash(label), squash(value)))
        })
        .collect();
    let (coverages, features) = lines(html);
    let discounts = scrape::elements(html, "span")
        .into_iter()
        .filter(|e| has_class(&e.tag, "discounts__item-name"))
        .map(|e| squash(&e.text))
        .collect();
    Some(PolicyDetail {
        number,
        address: text_with_class(html, "span", "policy-detail__heading-text").unwrap_or_default(),
        status: pill(html),
        kind,
        flood: html.contains("policy-summary__flood_endorsement"),
        header,
        terms: terms(html),
        deductibles: deductibles(html),
        coverages,
        features,
        discount_total: text_with_class(html, "span", "discounts__total"),
        discounts,
        lender: lender(html),
    })
}

// -------------------------------------------------------------- documents

/// One downloadable document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocItem {
    /// Kin's `data-document-label` (`declaration_page`), the stable id.
    pub label: String,
    pub name: String,
    pub description: Option<String>,
    /// The `active_storage` redirect, which answers with a pre-signed URL.
    pub url: String,
    /// The filename at the end of the URL.
    pub file: Option<String>,
}

/// The documents on `/kin_portal/policies/<number>/documents`.
pub fn documents(html: &str) -> Vec<DocItem> {
    scrape::blocks_with_class(html, "div", "policy-document__item")
        .into_iter()
        .filter_map(|item| {
            let link = scrape::elements(&item, "a")
                .into_iter()
                .find(|a| scrape::attr(&a.tag, "data-document-label").is_some())?;
            let label = scrape::attr(&link.tag, "data-document-label")?;
            let url = scrape::attr(&link.tag, "href")?;
            let name = text_with_class(&item, "p", "policy-document__name")
                .unwrap_or_else(|| label.replace('_', " "));
            let description = scrape::elements(&item, "p")
                .into_iter()
                .filter(|p| !has_class(&p.tag, "policy-document__name"))
                .map(|p| squash(&p.text))
                .find(|t| !t.is_empty());
            let file = url
                .split('?')
                .next()
                .and_then(|u| u.rsplit('/').next())
                .map(percent_decode)
                .filter(|f| !f.is_empty());
            Some(DocItem {
                label,
                name,
                description,
                url,
                file,
            })
        })
        .collect()
}

// --------------------------------------------------------------- payments

/// One billing transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentItem {
    /// `Policy payment 2 of 4`, `Refund payment`.
    pub description: String,
    pub amount: String,
    /// `scheduled`, `paid`, `issued` — the first word of the status line.
    pub status: Option<String>,
    pub date: Option<String>,
    pub card_last4: Option<String>,
}

/// The transactions on a payments page (`?billing_status=upcoming|approved`).
pub fn payments(html: &str) -> Vec<PaymentItem> {
    scrape::blocks_with_class(html, "div", "billing-transaction__item")
        .into_iter()
        .filter_map(|item| {
            let ps = scrape::elements(&item, "p");
            let description = ps
                .iter()
                .find(|p| has_class(&p.tag, "fw-bold") && !has_class(&p.tag, "m-l-auto"))
                .map(|p| squash(&p.text))?;
            let amount = ps
                .iter()
                .find(|p| {
                    has_class(&p.tag, "m-l-auto")
                        || scrape::attr(&p.tag, "data-testid").as_deref()
                            == Some("billing-transaction-amount")
                })
                .map(|p| squash(&p.text))?;
            let status_line = ps
                .iter()
                .find(|p| has_class(&p.tag, "fs-body-sm"))
                .map(|p| squash(&p.text))
                .unwrap_or_default();
            let status = status_line
                .split_whitespace()
                .next()
                .map(|w| w.to_ascii_lowercase());
            let after_on = status_line.split_once(" on ").map(|(_, r)| r);
            let date = after_on.and_then(|r| {
                // "Dec 1, 2026, •••• 0000" — the date is the first three words.
                let words: Vec<&str> = r.split_whitespace().take(3).collect();
                parse_month_date(&words.join(" "))
            });
            let card_last4 = status_line.split_once("••••").map(|(_, r)| {
                r.chars()
                    .filter(char::is_ascii_digit)
                    .take(4)
                    .collect::<String>()
            });
            Some(PaymentItem {
                description,
                amount,
                status,
                date,
                card_last4: card_last4.filter(|c| c.len() == 4),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_dates_parse_long_and_short_forms() {
        assert_eq!(
            parse_month_date("March 5, 2026").as_deref(),
            Some("2026-03-05")
        );
        assert_eq!(
            parse_month_date("Nov 30, 2026").as_deref(),
            Some("2026-11-30")
        );
        assert_eq!(parse_month_date("Smarch 1, 2026"), None);
        assert_eq!(parse_month_date("April 40, 2026"), None);
    }

    #[test]
    fn term_labels_yield_their_dates() {
        assert_eq!(
            term_dates("Current Term (03/05/2026 - 03/05/2027)"),
            (Some("2026-03-05".into()), Some("2027-03-05".into()))
        );
        assert_eq!(term_dates("Current Term"), (None, None));
    }

    #[test]
    fn percent_decoding_and_query_params() {
        assert_eq!(
            percent_decode("p......%40example.com"),
            "p......@example.com"
        );
        assert_eq!(percent_decode("Renewal%20Packet.pdf"), "Renewal Packet.pdf");
        assert_eq!(percent_decode("bad%zzescape%"), "bad%zzescape%");
        assert_eq!(
            query_param(
                "https://app.kin.com/users/verify_one_time_password?otp_contact=p......%40example.com&otp_method=email",
                "otp_contact"
            )
            .as_deref(),
            Some("p......@example.com")
        );
    }

    #[test]
    fn form_tokens_are_scoped_to_their_form() {
        let html = r#"
            <form action="/users/sign_out" method="post"><input type="hidden" name="authenticity_token" value="WRONG" /></form>
            <form action="/users/sign_in" method="post"><input type="hidden" name="authenticity_token" value="RIGHT" autocomplete="off" /></form>"#;
        assert_eq!(form_token(html, "/users/sign_in").as_deref(), Some("RIGHT"));
        assert_eq!(
            form_token(html, "/users/sign_out").as_deref(),
            Some("WRONG")
        );
        assert_eq!(form_token(html, "/nowhere"), None);
    }

    #[test]
    fn the_code_email_snippet_yields_its_code() {
        // Kin's wording; the shared extractor finds the lone six-digit run.
        let snippet = "kin.com Hi Pat, Your requested one-time password is: 123456 Please \
                       enter this password on your Customer Portal login page.";
        let code =
            pk_cli_auth::otp::extract_code(snippet, pk_cli_auth::otp::CodeShape::digits(6, 6))
                .unwrap();
        assert_eq!(code.expose(), "123456");
    }
}
