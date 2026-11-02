//! `kin` — read-only CLI for the Kin Insurance customer portal
//! (conforms to piekstra-cli/1).

use std::io::Write;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_auth::otp::CodeArgs;
use pk_cli_auth::{AuthMethod, AuthStatus, LoginArgs, LogoutArgs, SetCredentialArgs};
use pk_cli_config::ConfigStore;
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{output, resolve, CliError, CommonArgs, Paged, RangeArgs};
use pk_cli_documents::{fs_safe, verify_download, Document, DownloadBatch, SavedDocument};
use pk_cli_secrets::{CredentialStore, OnePassword, OpArgs, Secret};
use pk_cli_selfupdate::{SelfUpdateArgs, Updater};

use kin::bundle::{self, Vault};
use kin::config::{self, Config};
use kin::parse::{self, Term};
use kin::session::{self, Ctx, Portal, BIN};
use kin::{dto, jar::Jar};

const REPO: &str = "piekstra/kin-cli";

/// Read-only CLI for the Kin Insurance customer portal: policies, coverages,
/// documents, and payments (conforms to piekstra-cli/1).
#[derive(Parser, Debug)]
#[command(name = BIN, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Sign in, session status, sign out.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Non-secret settings.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Policies: list them, or show one's coverages and deductibles.
    #[command(subcommand)]
    Policies(PoliciesCmd),
    /// Policy documents (documents/v1 profile).
    #[command(subcommand)]
    Documents(DocumentsCmd),
    /// Scheduled and past payments.
    #[command(subcommand)]
    Payments(PaymentsCmd),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// Sign in with the password and a one-time code; store the session.
    Login(LoginCmd),
    /// Report credential and session state (auth-status/v1).
    Status(StatusArgs),
    /// Sign out; --forget also removes the stored password and settings.
    Logout(LogoutArgs),
    /// Store the password without signing in (rotation / headless setup).
    SetCredential(SetCredentialArgs),
}

#[derive(clap::Args, Debug)]
struct LoginCmd {
    /// The Kin login email (saved as `username`).
    #[arg(long, value_name = "EMAIL")]
    username: Option<String>,
    /// Where Kin sends the code: `email` or `text` (saved as `otp_channel`).
    #[arg(long, value_name = "CHANNEL")]
    channel: Option<String>,
    #[command(flatten)]
    code: CodeArgs,
    #[command(flatten)]
    login: LoginArgs,
    #[command(flatten)]
    op: OpArgs,
}

#[derive(clap::Args, Debug)]
struct StatusArgs {
    /// Check the stored session with one request to Kin (never signs in).
    #[arg(long)]
    verify: bool,
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Print the config file path.
    Path,
    /// Show the effective configuration.
    Show,
    /// Set a key (username, policy, otp_channel, mailbox, op_ref, secret_sources).
    Set { key: String, value: String },
    /// Remove a key.
    Unset { key: String },
}

/// `current` (default), `previous`, or a term's start date (YYYY-MM-DD).
#[derive(Debug, Clone, PartialEq, Eq)]
enum TermSel {
    Current,
    Previous,
    Starting(String),
}

fn parse_term(raw: &str) -> Result<TermSel, String> {
    match raw {
        "current" => Ok(TermSel::Current),
        "previous" => Ok(TermSel::Previous),
        d => pk_cli_core::dates::parse_iso(d)
            .map(|c| TermSel::Starting(pk_cli_core::dates::fmt_iso(c)))
            .map_err(|_| {
                format!("`{d}` is not a term (expected current, previous, or YYYY-MM-DD)")
            }),
    }
}

#[derive(clap::Args, Debug, Clone)]
struct PolicyArgs {
    /// Policy number, or part of its number or address (default: the
    /// configured `policy`, else the account's only policy).
    #[arg(long, value_name = "POLICY")]
    policy: Option<String>,
    /// Policy term: current, previous, or its start date (YYYY-MM-DD).
    #[arg(long, value_name = "TERM", value_parser = parse_term, default_value = "current")]
    term: TermSel,
}

#[derive(Subcommand, Debug)]
enum PoliciesCmd {
    /// List the account's policies (policy-list/v1).
    #[command(visible_alias = "ls")]
    List,
    /// Coverages, deductibles, premium and lender of one policy (policy/v1).
    Get {
        /// Policy number, or part of its number or address.
        policy: Option<String>,
        /// Policy term: current, previous, or its start date (YYYY-MM-DD).
        #[arg(long, value_name = "TERM", value_parser = parse_term, default_value = "current")]
        term: TermSel,
    },
}

#[derive(Subcommand, Debug)]
enum DocumentsCmd {
    /// List a policy term's documents (document-list/v1).
    #[command(visible_alias = "ls")]
    List {
        #[command(flatten)]
        policy: PolicyArgs,
        #[command(flatten)]
        range: RangeArgs,
    },
    /// Download one document, or --all into a directory (document-download/v1).
    #[command(visible_alias = "get")]
    Download {
        /// Document id (e.g. `declaration_page`) or part of its name.
        id: Option<String>,
        /// Download every document of the term into the -o directory.
        #[arg(long, conflicts_with = "id")]
        all: bool,
        /// Output file (`-` for stdout), or the directory with --all.
        #[arg(short, long, value_name = "PATH")]
        output: Option<String>,
        #[command(flatten)]
        policy: PolicyArgs,
    },
}

#[derive(Subcommand, Debug)]
enum PaymentsCmd {
    /// Upcoming payments, or past ones with --past (payment-list/v1).
    #[command(visible_alias = "ls")]
    List {
        /// Past (paid, refunded) transactions instead of upcoming ones.
        #[arg(long, conflicts_with = "all")]
        past: bool,
        /// Upcoming and past transactions.
        #[arg(long)]
        all: bool,
        /// Policy number, or part of its number or address.
        #[arg(long, value_name = "POLICY")]
        policy: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    let store = ConfigStore::new(BIN);
    match &cli.command {
        Command::Auth(cmd) => auth(cli, cmd, &store),
        Command::Config(cmd) => config_cmd(cli, cmd, &store),
        Command::Policies(cmd) => policies(cli, cmd, &store),
        Command::Documents(cmd) => documents(cli, cmd, &store),
        Command::Payments(cmd) => payments(cli, cmd, &store),
        Command::SelfUpdate(args) => Updater {
            repo: REPO.into(),
            binary: BIN.into(),
            target: env!("BUILD_TARGET").into(),
            current: env!("CARGO_PKG_VERSION").into(),
        }
        .run(args, cli.common.json, cli.common.quiet),
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
            Ok(())
        }
        Command::Info => {
            let info = CliInfo::new(
                BIN,
                env!("CARGO_PKG_VERSION"),
                &format!("https://github.com/{REPO}"),
                AuthInfo {
                    required: true,
                    method: "password".into(),
                    login_hint: Some(format!("{BIN} auth login")),
                },
                &["policies", "documents", "payments"],
            )
            .with_profiles(&[pk_cli_documents::PROFILE]);
            output::json(&serde_json::to_value(&info).expect("CliInfo serializes to JSON"));
            Ok(())
        }
    }
}

fn ctx<'a>(cli: &Cli, cfg: &'a Config, vault: &'a dyn Vault, account: String) -> Ctx<'a> {
    Ctx {
        vault,
        cfg,
        account,
        verbose: cli.common.verbose,
        quiet: cli.common.quiet,
        interactive: cli.common.interactive(),
    }
}

// ------------------------------------------------------------------- auth

fn auth(cli: &Cli, cmd: &AuthCmd, store: &ConfigStore) -> Result<(), CliError> {
    let mut cfg: Config = store.load()?;
    let creds = CredentialStore::for_binary(BIN);
    match cmd {
        AuthCmd::Login(args) => {
            // Everything that can be refused is checked before the keychain.
            let code = args.code.resolve()?;
            let username = args
                .username
                .as_deref()
                .map(config::validate_email)
                .transpose()?;
            let channel = args
                .channel
                .as_deref()
                .map(config::validate_channel)
                .transpose()?;
            if code.is_some() && (args.login.source.stdin || args.login.source.from_env.is_some()) {
                return Err(CliError::Usage(
                    "--code finishes a parked login; it takes no password source".into(),
                ));
            }
            let account = session::account(&cfg, username.as_deref())
                .map_err(|_| CliError::Usage("pass --username <EMAIL> the first time".into()))?;
            let mut changed = false;
            if username.is_some() && cfg.username != username {
                cfg.username = username.clone();
                changed = true;
            }
            if channel.is_some() && cfg.otp_channel != channel {
                cfg.otp_channel = channel.clone();
                changed = true;
            }
            if changed {
                store.save(&cfg)?;
            }
            let c = ctx(cli, &cfg, &creds, account.clone());

            if let Some(code) = code {
                let jar = session::login(&c, None, Some(&code))?;
                return finish_login(cli, &c, jar);
            }

            let stored = creds.load(&account)?.and_then(|b| b.password);
            let explicit = args.login.source.stdin
                || args.login.source.from_env.is_some()
                || args.op.op.is_some();
            let password = match stored {
                Some(p) if !explicit && !args.login.overwrite => Secret::new(p),
                _ => {
                    let op = match (&args.op.op, &cfg.op_ref) {
                        (None, Some(r)) if !explicit => OpArgs::reference(r.clone()),
                        _ => args.op.clone(),
                    };
                    let prompt =
                        (!args.login.non_interactive && c.interactive).then_some("Kin password");
                    let secret =
                        args.login
                            .source
                            .read_with_op(&op, &OnePassword::new(), prompt)?;
                    let keep = secret.expose().to_string();
                    bundle::update(&creds, &account, |b| b.password = Some(keep))?;
                    secret
                }
            };
            if args.login.no_verify {
                if !cli.common.quiet {
                    eprintln!("password stored in the OS keychain; not signed in (--no-verify)");
                }
                return Ok(());
            }
            let jar = session::login(&c, Some(password), None)?;
            finish_login(cli, &c, jar)
        }
        AuthCmd::Status(args) => {
            let account = cfg.username.clone();
            let mut status = AuthStatus::new(true, false, AuthMethod::Password);
            status.username = account.clone();
            if let Some(account) = account {
                let b = creds.load(&account)?.unwrap_or_default();
                status.credential_in_keychain = Some(b.password.is_some());
                status.authenticated = b.has_session();
                if b.parked.is_some() && !cli.common.quiet {
                    eprintln!("a login is waiting for its code: `{BIN} auth login --code <CODE>`");
                }
                if args.verify && b.has_session() {
                    let c = ctx(cli, &cfg, &creds, account);
                    let mut http = kin::client::Http::new(b.jar.unwrap_or_default(), c.verbose)?;
                    let page = http.get_following("/kin_portal")?;
                    let valid = page.status == 200 && parse::is_signed_in(&page.body);
                    status.session_valid = Some(valid);
                    status.authenticated = valid;
                    if valid {
                        let jar = http.jar.clone();
                        bundle::update(&creds, &c.account, |b| b.jar = Some(jar))?;
                    }
                }
            }
            status.emit(cli.common.json);
            Ok(())
        }
        AuthCmd::Logout(args) => {
            let Some(account) = cfg.username.clone() else {
                if !cli.common.quiet {
                    eprintln!("not logged in");
                }
                return Ok(());
            };
            let c = ctx(cli, &cfg, &creds, account.clone());
            let remote = Portal::open(&c)?.sign_out()?;
            if args.forget {
                creds.delete(&account)?;
                store.clear()?;
            }
            if !cli.common.quiet {
                eprintln!(
                    "{}{}",
                    if remote {
                        "signed out of Kin"
                    } else {
                        "session cleared"
                    },
                    if args.forget {
                        "; password and settings removed"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        AuthCmd::SetCredential(args) => {
            let account = session::account(&cfg, None)?;
            if creds.load(&account)?.and_then(|b| b.password).is_some() && !args.overwrite {
                return Err(CliError::Usage(
                    "a password is already stored; pass --overwrite to replace it".into(),
                ));
            }
            let secret = args.source.read(None)?;
            let keep = secret.expose().to_string();
            bundle::update(&creds, &account, |b| b.password = Some(keep))?;
            if !cli.common.quiet {
                eprintln!("password stored in the OS keychain");
            }
            Ok(())
        }
    }
}

fn finish_login(cli: &Cli, c: &Ctx, jar: Jar) -> Result<(), CliError> {
    bundle::update(c.vault, &c.account, |b| b.jar = Some(jar))?;
    let mut status = AuthStatus::new(true, true, AuthMethod::Password);
    status.username = Some(c.account.clone());
    status.credential_in_keychain =
        Some(c.vault.load(&c.account)?.and_then(|b| b.password).is_some());
    status.session_valid = Some(true);
    if cli.common.json {
        status.emit(true);
    } else if !cli.common.quiet {
        eprintln!("signed in to Kin as {}; session stored", c.account);
    }
    Ok(())
}

// ----------------------------------------------------------------- config

fn config_cmd(cli: &Cli, cmd: &ConfigCmd, store: &ConfigStore) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            println!("{}", store.path()?.display());
            Ok(())
        }
        ConfigCmd::Show => {
            let cfg: Config = store.load()?;
            let v = serde_json::to_value(&cfg).unwrap_or_default();
            if cli.common.json {
                output::json(&v);
            } else {
                output::render(&v);
            }
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg: Config = store.load()?;
            match key.as_str() {
                "username" => cfg.username = Some(config::validate_email(value)?),
                "policy" => cfg.policy = Some(value.trim().to_string()),
                "otp_channel" => cfg.otp_channel = Some(config::validate_channel(value)?),
                "mailbox" => {
                    config::parse_mailbox(value)?;
                    cfg.mailbox = Some(value.clone());
                }
                "op_ref" => cfg.op_ref = Some(value.parse().map_err(CliError::Usage)?),
                "secret_sources" => {
                    cfg.secret_sources = Some(value.parse().map_err(CliError::Usage)?)
                }
                other => {
                    return Err(CliError::Usage(format!(
                        "unknown config key `{other}` (known: {})",
                        config::KEYS
                    )))
                }
            }
            store.save(&cfg)
        }
        ConfigCmd::Unset { key } => {
            let mut cfg: Config = store.load()?;
            match key.as_str() {
                "username" => cfg.username = None,
                "policy" => cfg.policy = None,
                "otp_channel" => cfg.otp_channel = None,
                "mailbox" => cfg.mailbox = None,
                "op_ref" => cfg.op_ref = None,
                "secret_sources" => cfg.secret_sources = None,
                other => {
                    return Err(CliError::Usage(format!(
                        "unknown config key `{other}` (known: {})",
                        config::KEYS
                    )))
                }
            }
            store.save(&cfg)
        }
    }
}

// --------------------------------------------------------------- policies

/// The policy a command acts on.
fn pick_policy(
    portal: &mut Portal,
    cfg: &Config,
    explicit: Option<&str>,
) -> Result<String, CliError> {
    let wanted = explicit.map(str::to_string).or_else(|| cfg.policy.clone());
    if let Some(q) = &wanted {
        if q.to_ascii_uppercase().starts_with("KIN-") && !q.contains(' ') {
            return Ok(q.to_ascii_uppercase());
        }
    }
    let all = parse::policies(&portal.page("/kin_portal")?);
    if all.is_empty() {
        return Err(CliError::Upstream(
            "the Kin portal listed no policies (page changed?)".into(),
        ));
    }
    if let Some(q) = wanted {
        return resolve::pick(
            &all,
            &q,
            |p| vec![p.number.clone()],
            |p| &p.address,
            "policy",
        )
        .map(|p| p.number.clone());
    }
    let active: Vec<_> = all
        .iter()
        .filter(|p| p.status.as_deref() == Some("Active"))
        .collect();
    match (all.len(), active.len()) {
        (1, _) => Ok(all[0].number.clone()),
        (_, 1) => Ok(active[0].number.clone()),
        _ => Err(CliError::Usage(format!(
            "this account has {} policies ({}) — pass --policy, or `{BIN} config set policy <NUMBER>`",
            all.len(),
            all.iter().map(|p| p.number.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// Load `path` for the requested term: the page as served, re-requested
/// with `?term=` when another term is wanted. Returns the page and the term
/// it shows.
fn page_for_term(
    portal: &mut Portal,
    path: &str,
    want: &TermSel,
) -> Result<(String, Option<Term>), CliError> {
    let html = portal.page(path)?;
    let terms = parse::terms(&html);
    let selected = terms.iter().position(|t| t.selected);
    let target = match want {
        TermSel::Current => return Ok((html, selected.map(|i| terms[i].clone()))),
        TermSel::Previous => selected.and_then(|i| terms.get(i + 1)),
        TermSel::Starting(d) => terms
            .iter()
            .find(|t| t.start.as_deref() == Some(d.as_str())),
    }
    .cloned()
    .ok_or_else(|| {
        CliError::NotFound(format!(
            "no such term; this policy has: {}",
            terms
                .iter()
                .map(|t| t.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let html = portal.page(&format!("{path}?term={}", target.value))?;
    Ok((html, Some(target)))
}

fn policies(cli: &Cli, cmd: &PoliciesCmd, store: &ConfigStore) -> Result<(), CliError> {
    let cfg: Config = store.load()?;
    let account = session::account(&cfg, None)?;
    let creds = CredentialStore::for_binary(BIN);
    let c = ctx(cli, &cfg, &creds, account);
    let mut portal = Portal::open(&c)?;
    let result = (|| match cmd {
        PoliciesCmd::List => {
            let html = portal.page("/kin_portal")?;
            let items: Vec<_> = parse::policies(&html)
                .iter()
                .map(dto::policy_summary)
                .collect();
            if items.is_empty() {
                return Err(CliError::Upstream(
                    "the Kin portal listed no policies (page changed?)".into(),
                ));
            }
            output::emit_list(
                cli.common.json,
                "policy",
                items,
                &["number", "status", "kind", "address", "flood_endorsement"],
            );
            Ok(())
        }
        PoliciesCmd::Get { policy, term } => {
            let number = pick_policy(&mut portal, &cfg, policy.as_deref())?;
            let path = format!("/kin_portal/policies/{number}/details");
            let (html, selected) = page_for_term(&mut portal, &path, term)?;
            let detail = parse::policy_detail(&html).ok_or_else(|| {
                CliError::Upstream(format!("could not read {number}'s details (page changed?)"))
            })?;
            let v = dto::policy_detail(&detail, selected.as_ref());
            output::emit(cli.common.json, "policy", v, dto::render_policy);
            Ok(())
        }
    })();
    portal.save()?;
    result
}

// -------------------------------------------------------------- documents

fn documents(cli: &Cli, cmd: &DocumentsCmd, store: &ConfigStore) -> Result<(), CliError> {
    // Argument checks first: none of these may reach the keychain.
    match cmd {
        DocumentsCmd::List { range, .. } => range.validate()?,
        DocumentsCmd::Download {
            id, all, output, ..
        } => {
            if id.is_none() && !all {
                return Err(CliError::Usage(
                    "name a document id, or pass --all -o <DIR>".into(),
                ));
            }
            if *all && output.is_none() {
                return Err(CliError::Usage("--all needs -o <DIR>".into()));
            }
            if *all && output.as_deref() == Some("-") {
                return Err(CliError::Usage(
                    "--all writes files; -o must be a directory".into(),
                ));
            }
        }
    }
    let cfg: Config = store.load()?;
    let account = session::account(&cfg, None)?;
    let creds = CredentialStore::for_binary(BIN);
    let c = ctx(cli, &cfg, &creds, account);
    let mut portal = Portal::open(&c)?;
    let result = (|| {
        let policy = match cmd {
            DocumentsCmd::List { policy, .. } | DocumentsCmd::Download { policy, .. } => policy,
        };
        let number = pick_policy(&mut portal, &cfg, policy.policy.as_deref())?;
        let path = format!("/kin_portal/policies/{number}/documents");
        let (html, term) = page_for_term(&mut portal, &path, &policy.term)?;
        let items = parse::documents(&html);
        let date = term.as_ref().and_then(|t| t.start.clone());
        let docs: Vec<(Document, String)> = items
            .iter()
            .map(|d| {
                let mut doc = Document::new(&d.label, &d.name);
                doc.date = date.clone();
                doc.category = Some("policy".into());
                doc.file = d.file.clone();
                (doc, d.url.clone())
            })
            .collect();
        match cmd {
            DocumentsCmd::List { range, .. } => {
                let mut list: Vec<Document> = docs
                    .into_iter()
                    .map(|(d, _)| d)
                    .filter(|d| match (&range.since, &d.date) {
                        (Some(s), Some(dt)) => dt.as_str() >= s.as_str(),
                        _ => true,
                    })
                    .filter(|d| match (&range.until, &d.date) {
                        (Some(u), Some(dt)) => dt.as_str() <= u.as_str(),
                        _ => true,
                    })
                    .collect();
                if let Some(n) = range.limit {
                    list.truncate(n as usize);
                }
                Paged::new("document", list).emit(cli.common.json);
                Ok(())
            }
            DocumentsCmd::Download {
                id, all, output, ..
            } => {
                if docs.is_empty() {
                    return Err(CliError::NotFound(format!(
                        "{number} lists no documents for this term"
                    )));
                }
                if *all {
                    let dir = std::path::PathBuf::from(output.as_deref().unwrap_or("."));
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| CliError::Other(format!("creating {}: {e}", dir.display())))?;
                    let mut saved = Vec::new();
                    let mut skipped = Vec::new();
                    for (doc, url) in &docs {
                        match portal.download(url).and_then(|bytes| {
                            verify_download(&bytes, Some("pdf"))?;
                            Ok(bytes)
                        }) {
                            Ok(bytes) => {
                                let path = dir.join(file_name(doc));
                                write_private(&path, &bytes)?;
                                saved.push(SavedDocument::from_document(
                                    doc,
                                    path.display().to_string(),
                                    bytes.len() as u64,
                                ));
                            }
                            Err(e) => skipped.push(
                                pk_cli_documents::SkippedDocument::new(&doc.id, e.to_string())
                                    .with_code(match e {
                                        CliError::Upstream(_) => pk_cli_documents::SKIP_UPSTREAM,
                                        _ => pk_cli_documents::SKIP_VERIFY_FAILED,
                                    }),
                            ),
                        }
                    }
                    let batch =
                        DownloadBatch::new(dir.display().to_string(), saved).with_skipped(skipped);
                    pk_cli_documents::emit(&batch, cli.common.json);
                    return Ok(());
                }
                let q = id.as_deref().unwrap_or_default();
                let (doc, url) = resolve::pick(
                    &docs,
                    q,
                    |d| vec![d.0.id.clone()],
                    |d| &d.0.name,
                    "document",
                )?;
                let bytes = portal.download(url)?;
                verify_download(&bytes, Some("pdf"))?;
                let target = output.clone().unwrap_or_else(|| file_name(doc));
                if target == "-" {
                    return match std::io::stdout().write_all(&bytes) {
                        // The reader stopped early (`| head`): its choice, not
                        // a failure.
                        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                        other => other.map_err(|e| CliError::Other(format!("writing stdout: {e}"))),
                    };
                }
                let path = std::path::PathBuf::from(&target);
                write_private(&path, &bytes)?;
                let saved = SavedDocument::from_document(
                    doc,
                    path.display().to_string(),
                    bytes.len() as u64,
                );
                pk_cli_documents::emit(&saved, cli.common.json);
                Ok(())
            }
        }
    })();
    portal.save()?;
    result
}

/// The portal's filename, made safe to join to a path. `fs_safe` rewrites
/// dots too, so the extension is split off first.
fn file_name(doc: &Document) -> String {
    let raw = doc
        .file
        .clone()
        .unwrap_or_else(|| format!("{}.pdf", doc.id));
    let stem = match raw.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("pdf") => stem.to_string(),
        _ => raw,
    };
    format!("{}.pdf", fs_safe(&stem))
}

/// Policy documents carry names, addresses and loan numbers: owner-only.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), CliError> {
    use std::fs::OpenOptions;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))?;
    f.write_all(bytes)
        .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))
}

// --------------------------------------------------------------- payments

fn payments(cli: &Cli, cmd: &PaymentsCmd, store: &ConfigStore) -> Result<(), CliError> {
    let PaymentsCmd::List { past, all, policy } = cmd;
    let cfg: Config = store.load()?;
    let account = session::account(&cfg, None)?;
    let creds = CredentialStore::for_binary(BIN);
    let c = ctx(cli, &cfg, &creds, account);
    let mut portal = Portal::open(&c)?;
    let result = (|| {
        let number = pick_policy(&mut portal, &cfg, policy.as_deref())?;
        let base = format!("/kin_portal/policies/{number}/payments");
        let mut views = Vec::new();
        if !past || *all {
            views.push("upcoming");
        }
        if *past || *all {
            views.push("approved");
        }
        let mut items = Vec::new();
        for view in views {
            let html = portal.page(&format!("{base}?billing_status={view}"))?;
            items.extend(parse::payments(&html).iter().map(dto::payment));
        }
        if cli.common.json {
            output::emit_list(true, "payment", items, &[]);
        } else {
            let rows: Vec<_> = items.iter().map(dto::payment_row).collect();
            output::table(&output::table_view(
                &rows,
                &["date", "description", "amount", "status", "card_last4"],
            ));
        }
        Ok(())
    })();
    portal.save()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_keep_one_pdf_extension() {
        let mut doc = Document::new("declaration_page", "Declaration page");
        doc.file = Some("declaration_page.pdf".into());
        assert_eq!(file_name(&doc), "declaration_page.pdf");
        doc.file = Some("KIN-DP-FL-000000001 Renewal Packet.pdf".into());
        let name = file_name(&doc);
        assert!(name.ends_with("Renewal_Packet.pdf"), "{name}");
        assert!(!name.contains('/') && !name.contains(' '));
        doc.file = Some("../../etc/passwd".into());
        assert!(!file_name(&doc).contains('/'));
        doc.file = None;
        assert_eq!(file_name(&doc), "declaration_page.pdf");
    }

    #[test]
    fn terms_parse_their_three_forms() {
        assert_eq!(parse_term("current").unwrap(), TermSel::Current);
        assert_eq!(parse_term("previous").unwrap(), TermSel::Previous);
        assert_eq!(
            parse_term("2025-04-17").unwrap(),
            TermSel::Starting("2025-04-17".into())
        );
        assert!(parse_term("last").is_err());
    }
}
