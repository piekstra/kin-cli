//! Signing in, and reading the portal with a stored session.
//!
//! A read replays the stored session. When Kin has let it lapse, the read
//! signs in again once — password from the configured sources, code from
//! the mailbox when one is configured, else a prompt — and retries. Without
//! a terminal or a mailbox that ends in exit 3 with the parked login waiting
//! for `kin auth login --code`.

use pk_cli_auth::otp::{CodeShape, GroMailbox, MailQuery, OtpCode, OtpLogin};
use pk_cli_core::CliError;
use pk_cli_secrets::{Secret, SecretResolver, SecretSpec};

use crate::bundle::{self, ParkedView, PasswordView, Vault};
use crate::client::{path_of, Http, KinLogin};
use crate::config::{parse_mailbox, Config, MailboxSetting};
use crate::jar::Jar;
use crate::parse;

pub const BIN: &str = "kin";

/// The env var a password may come from (SPEC §1.7 precedence).
pub const PASSWORD_ENV: &str = "KIN_PASSWORD";

/// Kin's code email: `notification@help.kin.com`, "Requested One-Time
/// Password". Its snippet carries the six-digit code.
pub const CODE_QUERY: &str = "from:help.kin.com subject:\"One-Time Password\"";

/// gro's profile selector. gro 2.x removed the `--ref` flag that
/// `GroMailbox::credential_ref` passes, while both 1.x and 2.x read this
/// variable, so a named profile is selected through it.
const GRO_PROFILE_ENV: &str = "GOOGLE_READONLY_CREDENTIAL_REF";

/// What every command that talks to Kin needs.
pub struct Ctx<'a> {
    pub vault: &'a dyn Vault,
    pub cfg: &'a Config,
    pub account: String,
    pub verbose: bool,
    pub quiet: bool,
    pub interactive: bool,
}

impl Ctx<'_> {
    fn note(&self, msg: &str) {
        if !self.quiet {
            eprintln!("{msg}");
        }
    }
}

/// The keychain account: `--username`, else the configured username.
pub fn account(cfg: &Config, explicit: Option<&str>) -> Result<String, CliError> {
    explicit
        .map(str::to_string)
        .or_else(|| cfg.username.clone())
        .ok_or_else(|| {
            CliError::Auth(format!(
                "no Kin login configured — run `{BIN} auth login --username <EMAIL>`"
            ))
        })
}

/// The password from the configured sources: `KIN_PASSWORD`, the keychain
/// bundle, then 1Password (`op_ref`), in `secret_sources` order.
pub fn resolve_password(ctx: &Ctx) -> Result<Option<Secret>, CliError> {
    let spec = SecretSpec::new(&ctx.account)
        .env(PASSWORD_ENV)
        .op(ctx.cfg.op_ref.clone());
    let view = PasswordView { vault: ctx.vault };
    Ok(SecretResolver::new(&view)
        .order(ctx.cfg.secret_sources.clone().unwrap_or_default())
        .resolve(&spec)?
        .map(|r| {
            if ctx.verbose {
                eprintln!("password from {}", r.source);
            }
            r.secret
        }))
}

/// The configured mailbox, if any.
fn mailbox(cfg: &Config) -> Result<Option<GroMailbox>, CliError> {
    let Some(setting) = cfg.mailbox.as_deref() else {
        return Ok(None);
    };
    match parse_mailbox(setting)? {
        MailboxSetting::None => Ok(None),
        MailboxSetting::Gro(profile) => {
            if let Some(p) = profile {
                // Single-threaded, set before any child process starts.
                std::env::set_var(GRO_PROFILE_ENV, format!("google-readonly/{p}"));
            }
            Ok(Some(GroMailbox::new()))
        }
    }
}

/// The one-time-code login: start one (password given) or finish the parked
/// one (`code` given). Returns the signed-in jar.
pub fn login(ctx: &Ctx, password: Option<Secret>, code: Option<&OtpCode>) -> Result<Jar, CliError> {
    let channel = ctx.cfg.channel().to_string();
    let transport = KinLogin {
        email: ctx.account.clone(),
        password,
        channel: channel.clone(),
        verbose: ctx.verbose,
    };
    let slot = ParkedView {
        vault: ctx.vault,
        account: &ctx.account,
    };
    let gro = mailbox(ctx.cfg)?;
    let mut flow = OtpLogin::new(BIN, &transport, &slot)
        .interactive(ctx.interactive)
        .quiet(ctx.quiet);
    if let (Some(gro), "email") = (&gro, channel.as_str()) {
        let query = MailQuery::new(CODE_QUERY)?.code_shape(CodeShape::digits(6, 6));
        flow = flow.mailbox(gro, query);
    }
    flow.login(code)
}

/// A portal session for one command.
pub struct Portal<'a> {
    ctx: &'a Ctx<'a>,
    http: Http,
    /// Whether a stored session existed when the command started. Kin sets
    /// a fresh session cookie even on the anonymous redirect to sign-in, so
    /// the jar alone cannot tell "lapsed" from "never signed in".
    had_session: bool,
    relogged: bool,
}

impl<'a> Portal<'a> {
    pub fn open(ctx: &'a Ctx<'a>) -> Result<Self, CliError> {
        let jar = ctx
            .vault
            .load(&ctx.account)?
            .and_then(|b| b.jar)
            .unwrap_or_default();
        Ok(Portal {
            ctx,
            had_session: jar.has_session(),
            http: Http::new(jar, ctx.verbose)?,
            relogged: false,
        })
    }

    /// A signed-in page's HTML, signing in again once if the session lapsed.
    pub fn page(&mut self, path: &str) -> Result<String, CliError> {
        let mut page = self.http.get_following(path)?;
        if page.needs_login() {
            self.relogin()?;
            page = self.http.get_following(path)?;
            if page.needs_login() {
                return Err(CliError::Auth(format!(
                    "Kin sent the new session back to sign-in — run `{BIN} auth login`"
                )));
            }
        }
        if page.status == 404 {
            return Err(CliError::NotFound(format!(
                "Kin has no page at {}",
                path_of(path)
            )));
        }
        if page.status != 200 {
            return Err(CliError::Upstream(format!(
                "Kin answered HTTP {} for {}",
                page.status,
                path_of(path)
            )));
        }
        if !parse::is_signed_in(&page.body) {
            return Err(CliError::Upstream(format!(
                "{} did not render as a signed-in page (page changed?)",
                path_of(path)
            )));
        }
        Ok(page.body)
    }

    fn relogin(&mut self) -> Result<(), CliError> {
        if self.relogged {
            return Err(CliError::Auth(
                "the Kin session lapsed again right after signing in".into(),
            ));
        }
        self.relogged = true;
        if self.had_session {
            self.ctx.note("the Kin session lapsed; signing in again");
        } else {
            self.ctx.note("no stored Kin session; signing in");
        }
        let password = resolve_password(self.ctx)?.ok_or_else(|| {
            CliError::Auth(format!(
                "not signed in to Kin and no password is stored — run `{BIN} auth login`"
            ))
        })?;
        self.http.jar = login(self.ctx, Some(password), None)?;
        self.save()
    }

    /// Download through the session (the cookie goes to Kin only).
    pub fn download(&mut self, url: &str) -> Result<Vec<u8>, CliError> {
        match self.http.download(url) {
            Err(CliError::Auth(_)) if !self.relogged => {
                self.relogin()?;
                Ok(self.http.download(url)?.0)
            }
            other => other.map(|(bytes, _)| bytes),
        }
    }

    /// Write the (rotated) session back to the keychain bundle.
    pub fn save(&self) -> Result<(), CliError> {
        let jar = self.http.jar.clone();
        bundle::update(self.ctx.vault, &self.ctx.account, |b| b.jar = Some(jar))
    }

    pub fn jar(&self) -> &Jar {
        &self.http.jar
    }

    /// Sign out at Kin (best effort) and forget the session locally.
    pub fn sign_out(&mut self) -> Result<bool, CliError> {
        let mut remote = false;
        if self.http.jar.has_session() {
            if let Ok(home) = self.http.get_following("/kin_portal") {
                if let Some(token) = parse::form_token(&home.body, "/users/sign_out") {
                    remote = self
                        .http
                        .post_form(
                            "/users/sign_out",
                            &[("_method", "delete"), ("authenticity_token", &token)],
                        )
                        .is_ok_and(|p| p.is_redirect());
                }
            }
        }
        bundle::update(self.ctx.vault, &self.ctx.account, |b| b.jar = None)?;
        Ok(remote)
    }
}
