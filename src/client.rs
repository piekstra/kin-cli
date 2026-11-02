//! HTTP to `app.kin.com`, and the login that earns a session.
//!
//! Redirects are followed by hand, not by reqwest: a lapsed session shows up
//! as a redirect to `/users/sign_in`, which a client that follows redirects
//! silently turns into a 200 login page; and a document download redirects
//! to a pre-signed S3 URL, which must never be sent the Kin session cookie.
//! Cookies come from [`Jar`], so the session survives between runs.

use pk_cli_auth::otp::{CodeSent, OtpCode, OtpTransport};
use pk_cli_core::CliError;
use pk_cli_secrets::Secret;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};

use crate::jar::Jar;
use crate::parse;

pub const BASE: &str = "https://app.kin.com";
const BIN: &str = "kin";

/// Hops a redirect chain may take before it is treated as a loop.
const MAX_HOPS: usize = 6;

/// One response, its body read.
#[derive(Debug)]
pub struct Page {
    pub status: u16,
    /// The URL that answered (after any redirects followed).
    pub url: String,
    /// An absolute `Location`, on a redirect.
    pub location: Option<String>,
    pub body: String,
}

impl Page {
    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status) && self.location.is_some()
    }

    /// The session is gone: Kin sent us to the sign-in page, or rendered it.
    pub fn needs_login(&self) -> bool {
        self.location
            .as_deref()
            .is_some_and(|l| path_of(l).starts_with("/users/sign_in"))
            || path_of(&self.url).starts_with("/users/sign_in")
            || parse::is_login_page(&self.body)
    }
}

/// The path of a URL, without scheme, host or query. Used for logs and
/// errors, which must never carry a query: a document link's query is a
/// pre-signed credential.
pub fn path_of(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .map(|r| r.find('/').map(|i| &r[i..]).unwrap_or("/"))
        .unwrap_or(url);
    rest.split(['?', '#']).next().unwrap_or(rest)
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

fn is_kin(url: &str) -> bool {
    host_of(url) == Some("app.kin.com")
}

fn absolute(path_or_url: &str) -> String {
    if path_or_url.starts_with("https://") || path_or_url.starts_with("http://") {
        path_or_url.to_string()
    } else {
        format!("{BASE}{path_or_url}")
    }
}

fn upstream(context: &str, e: reqwest::Error) -> CliError {
    // `without_url`: reqwest puts the full URL in its errors, query included.
    CliError::Upstream(format!("{context}: {}", e.without_url()))
}

/// A blocking client with the session jar.
pub struct Http {
    client: Client,
    pub jar: Jar,
    verbose: bool,
}

impl Http {
    pub fn new(jar: Jar, verbose: bool) -> Result<Self, CliError> {
        let client = pk_cli_http::builder(BIN, env!("CARGO_PKG_VERSION"))
            .cookie_store(false)
            .redirect(Policy::none())
            .build()
            .map_err(|e| CliError::Other(format!("building the HTTP client: {e}")))?;
        Ok(Http {
            client,
            jar,
            verbose,
        })
    }

    fn send(&mut self, method: &str, rb: RequestBuilder, url: &str) -> Result<Page, CliError> {
        let rb = match (is_kin(url), self.jar.header()) {
            (true, Some(cookie)) => rb.header(COOKIE, cookie),
            _ => rb,
        };
        let resp = rb
            .send()
            .map_err(|e| upstream(&format!("{method} {}", path_of(url)), e))?;
        if is_kin(url) {
            self.jar.absorb(resp.headers());
        }
        let status = resp.status().as_u16();
        let location = resp
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(absolute);
        let body = resp
            .text()
            .map_err(|e| upstream(&format!("reading {}", path_of(url)), e))?;
        if self.verbose {
            eprintln!("{method} {} -> {status}", path_of(url));
        }
        Ok(Page {
            status,
            url: url.to_string(),
            location,
            body,
        })
    }

    /// One GET, redirects not followed.
    pub fn get(&mut self, path: &str) -> Result<Page, CliError> {
        let url = absolute(path);
        let rb = self.client.get(&url);
        self.send("GET", rb, &url)
    }

    /// One form POST, redirects not followed.
    pub fn post_form(&mut self, path: &str, fields: &[(&str, &str)]) -> Result<Page, CliError> {
        let url = absolute(path);
        let rb = self.client.post(&url).form(fields);
        self.send("POST", rb, &url)
    }

    /// GET, following redirects within `app.kin.com`. Stops at (and returns)
    /// a redirect to the sign-in page, so the caller can see the session
    /// lapsed rather than a rendered login form.
    pub fn get_following(&mut self, path: &str) -> Result<Page, CliError> {
        let mut page = self.get(path)?;
        for _ in 0..MAX_HOPS {
            if !page.is_redirect() || page.needs_login() {
                return Ok(page);
            }
            let next = page.location.clone().unwrap_or_default();
            if !is_kin(&next) {
                return Err(CliError::Upstream(format!(
                    "Kin redirected {} off the portal",
                    path_of(&page.url)
                )));
            }
            page = self.get(&next)?;
        }
        Err(CliError::Upstream(format!(
            "too many redirects from {}",
            path_of(path)
        )))
    }

    /// Download a document: the portal link answers with a redirect to a
    /// pre-signed S3 URL. The session cookie goes only to `app.kin.com`;
    /// every hop must be https. Returns the bytes and the content type.
    pub fn download(&mut self, url: &str) -> Result<(Vec<u8>, Option<String>), CliError> {
        let mut current = absolute(url);
        for _ in 0..MAX_HOPS {
            if !current.starts_with("https://") {
                return Err(CliError::Upstream(
                    "a document link redirected to a non-https URL".into(),
                ));
            }
            let mut rb = self.client.get(&current);
            if is_kin(&current) {
                if let Some(cookie) = self.jar.header() {
                    rb = rb.header(COOKIE, cookie);
                }
            }
            let resp = rb
                .send()
                .map_err(|e| upstream(&format!("downloading {}", path_of(&current)), e))?;
            if is_kin(&current) {
                self.jar.absorb(resp.headers());
            }
            let status = resp.status().as_u16();
            if self.verbose {
                eprintln!(
                    "GET {}{} -> {status}",
                    host_of(&current).unwrap_or(""),
                    path_of(&current)
                );
            }
            if (300..400).contains(&status) {
                let next = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(absolute)
                    .ok_or_else(|| {
                        CliError::Upstream("a document redirect had no Location".into())
                    })?;
                if path_of(&next).starts_with("/users/sign_in") {
                    return Err(CliError::Auth(
                        "the Kin session lapsed during the download".into(),
                    ));
                }
                current = next;
                continue;
            }
            if status != 200 {
                return Err(CliError::Upstream(format!(
                    "document download answered HTTP {status} at {}",
                    host_of(&current).unwrap_or("?")
                )));
            }
            let ctype = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let bytes = resp
                .bytes()
                .map_err(|e| upstream("reading the document", e))?;
            return Ok((bytes.to_vec(), ctype));
        }
        Err(CliError::Upstream(
            "too many redirects downloading a document".into(),
        ))
    }
}

// ------------------------------------------------------------------ login

/// What is parked while a login waits for its code: the half-signed-in jar
/// and the CSRF token of the verify form.
#[derive(Serialize, Deserialize)]
struct InFlight {
    jar: Jar,
    token: String,
}

/// The provider side of the one-time-code login (`pk_cli_auth::otp`).
///
/// `request_code` signs in with the password and asks Kin to send a code;
/// `redeem` presents the code. The flow, the parking and the mailbox read
/// are the shared crate's.
pub struct KinLogin {
    pub email: String,
    /// `None` when only resuming a parked login with `--code`.
    pub password: Option<Secret>,
    /// `email` or `text`.
    pub channel: String,
    pub verbose: bool,
}

impl KinLogin {
    fn expect_redirect_to(page: &Page, path: &str, step: &str) -> Result<String, CliError> {
        match &page.location {
            Some(loc) if path_of(loc).starts_with(path) => Ok(loc.clone()),
            Some(loc) => Err(CliError::Upstream(format!(
                "{step}: Kin redirected to {} instead of {path}",
                path_of(loc)
            ))),
            None => Err(CliError::Upstream(format!(
                "{step}: Kin answered HTTP {} instead of redirecting to {path}",
                page.status
            ))),
        }
    }
}

impl OtpTransport for KinLogin {
    type Session = Jar;

    fn request_code(&self) -> Result<CodeSent, CliError> {
        let password = self.password.as_ref().ok_or_else(|| {
            CliError::Auth(format!("no Kin password available; run `{BIN} auth login`"))
        })?;
        let mut http = Http::new(Jar::new(), self.verbose)?;

        let page = http.get("/users/sign_in")?;
        let token = parse::form_token(&page.body, "/users/sign_in").ok_or_else(|| {
            CliError::Upstream("the Kin sign-in page has no login form (page changed?)".into())
        })?;
        let page = http.post_form(
            "/users/sign_in",
            &[
                ("authenticity_token", &token),
                ("user[email]", &self.email),
                ("user[password]", password.expose()),
                ("user[remember_me]", "0"),
                ("commit", "Log in"),
            ],
        )?;
        if page.location.is_none() && parse::is_login_page(&page.body) {
            let why = parse::form_error(&page.body).unwrap_or_default();
            return Err(CliError::Auth(format!(
                "Kin rejected the email or password{}{why}",
                if why.is_empty() { "" } else { ": " }
            )));
        }
        if page
            .location
            .as_deref()
            .is_some_and(|l| path_of(l).starts_with("/kin_portal"))
        {
            // Never observed: Kin has always asked for a code. Say so plainly
            // rather than guess at a flow nobody has seen.
            return Err(CliError::Upstream(
                "Kin signed in without asking for a one-time code; this CLI expects the \
                 code step — please report it"
                    .into(),
            ));
        }
        Self::expect_redirect_to(&page, "/users/one_time_password", "signing in")?;

        let page = http.get("/users/one_time_password")?;
        let token = parse::form_token(&page.body, "/users/one_time_password").ok_or_else(|| {
            CliError::Upstream("the Kin code page has no delivery form (page changed?)".into())
        })?;
        let channels = parse::otp_channels(&page.body);
        if !channels.iter().any(|c| c.method == self.channel) {
            let offered: Vec<&str> = channels.iter().map(|c| c.method.as_str()).collect();
            return Err(CliError::Usage(format!(
                "Kin does not offer `{}` delivery for this account (offered: {})",
                self.channel,
                if offered.is_empty() {
                    "none found".to_string()
                } else {
                    offered.join(", ")
                }
            )));
        }
        let page = http.post_form(
            "/users/one_time_password",
            &[
                ("authenticity_token", &token),
                ("otp_method", &self.channel),
                ("commit", "Send code"),
            ],
        )?;
        let verify_url =
            Self::expect_redirect_to(&page, "/users/verify_one_time_password", "sending the code")?;
        let page = http.get(&verify_url)?;
        let token =
            parse::form_token(&page.body, "/users/verify_one_time_password").ok_or_else(|| {
                CliError::Upstream("the Kin verify page has no code form (page changed?)".into())
            })?;

        let in_flight = serde_json::to_string(&InFlight {
            jar: http.jar.clone(),
            token,
        })
        .map_err(|e| CliError::Other(format!("serializing the parked login: {e}")))?;
        let mut sent = CodeSent::new(in_flight);
        sent.channel = Some(self.channel.clone());
        sent.sent_to_hint = parse::query_param(&verify_url, "otp_contact");
        Ok(sent)
    }

    fn redeem(&self, in_flight: &str, code: &OtpCode) -> Result<Jar, CliError> {
        let parked: InFlight = serde_json::from_str(in_flight).map_err(|_| {
            CliError::Auth(format!(
                "the parked login is unreadable; run `{BIN} auth login` again"
            ))
        })?;
        let mut http = Http::new(parked.jar, self.verbose)?;
        let page = http.post_form(
            "/users/verify_one_time_password",
            &[
                ("authenticity_token", &parked.token),
                ("otp_attempt", code.expose()),
                ("commit", "Submit"),
            ],
        )?;
        match page.location.as_deref().map(path_of) {
            Some(p) if p.starts_with("/kin_portal") => {}
            Some(p) if p.starts_with("/users/sign_in") => {
                return Err(CliError::Auth(
                    "Kin expired the login waiting for a code".into(),
                ))
            }
            _ => {
                let why = parse::form_error(&page.body)
                    .unwrap_or_else(|| "Kin rejected the code".to_string());
                return Err(CliError::Auth(why));
            }
        }
        // A redirect is not proof: require the signed-in portal itself.
        let home = http.get_following("/kin_portal")?;
        if home.status != 200 || !parse::is_signed_in(&home.body) {
            return Err(CliError::Upstream(
                "Kin accepted the code but the portal did not load signed in".into(),
            ));
        }
        Ok(http.jar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_never_carry_queries_or_hosts() {
        assert_eq!(
            path_of(
                "https://kin-documents-production.s3.amazonaws.com/a/b.pdf?X-Amz-Signature=secret"
            ),
            "/a/b.pdf"
        );
        assert_eq!(path_of("https://app.kin.com"), "/");
        assert_eq!(path_of("/users/sign_in?x=1"), "/users/sign_in");
    }

    #[test]
    fn only_the_portal_host_counts_as_kin() {
        assert!(is_kin("https://app.kin.com/kin_portal"));
        assert!(!is_kin("http://app.kin.com/kin_portal"));
        assert!(!is_kin("https://app.kin.com.evil.example/"));
        assert!(!is_kin(
            "https://kin-documents-production.s3.amazonaws.com/x"
        ));
    }

    #[test]
    fn a_sign_in_redirect_or_page_means_the_session_lapsed() {
        let redirect = Page {
            status: 302,
            url: format!("{BASE}/kin_portal"),
            location: Some(format!("{BASE}/users/sign_in")),
            body: String::new(),
        };
        assert!(redirect.needs_login());
        let rendered = Page {
            status: 200,
            url: format!("{BASE}/kin_portal"),
            location: None,
            body: r#"<input type="password" name="user[password]">"#.into(),
        };
        assert!(rendered.needs_login());
        let fine = Page {
            status: 200,
            url: format!("{BASE}/kin_portal"),
            location: None,
            body: r#"<form action="/users/sign_out">"#.into(),
        };
        assert!(!fine.needs_login());
    }
}
