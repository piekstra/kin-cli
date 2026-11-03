//! Every tool over the real read path against a stub portal on 127.0.0.1, so what goes
//! over the wire is what is checked: GETs to the read pages, and never a password, a
//! sign-in, a form or a write to the keychain. Offline and keychain-free: the vault is
//! in memory and the host is an argument of `LivePortal::new`, which no setting or
//! variable reaches.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use kin::bundle::{Bundle, MemVault, Vault};
use kin::jar::{Jar, SESSION_COOKIE};
use kin::mcp::{KinTools, LivePortal};
use pk_cli_config::ConfigStore;
use pk_cli_core::CliError;
use pk_cli_mcp::conformance::{assert_fits, call};
use pk_cli_mcp::{CallToolResult, Toolset};
use serde_json::{json, Value};

const ACCOUNT: &str = "someone@example.com";
const PASSWORD: &str = "dummy-password-for-tests";
const STORED: &str = "stored-session";

/// Everything a tool may read, by method, path and query.
const READS: [&str; 7] = [
    "GET /kin_portal",
    "GET /kin_portal/policies/KIN-DP-FL-000000001/details",
    "GET /kin_portal/policies/KIN-DP-FL-000000001/details?term=0",
    "GET /kin_portal/policies/KIN-DP-FL-000000001/documents",
    "GET /kin_portal/policies/KIN-DP-FL-000000001/documents?term=0",
    "GET /kin_portal/policies/KIN-DP-FL-000000001/payments?billing_status=upcoming",
    "GET /kin_portal/policies/KIN-DP-FL-000000001/payments?billing_status=approved",
];

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The page with its `previous` term shown, as Kin serves it for `?term=0`.
fn showing_previous(html: String) -> String {
    let bare = html
        .replace(" selected>", ">")
        .replace("value=\"0\" >", "value=\"0\" selected>");
    if bare.contains("value=\"0\" selected") {
        return bare;
    }
    bare.replace("value=\"current_term\"", "value=\"0\" selected")
}

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<String>,
    body: String,
}

fn page(body: String) -> Reply {
    Reply {
        status: 200,
        headers: Vec::new(),
        body,
    }
}

/// What the portal answers, by default.
fn fixtures(target: &str) -> Reply {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let policy = "/kin_portal/policies/KIN-DP-FL-000000001";
    match (path.strip_prefix(policy), query) {
        (None, _) if path == "/kin_portal" => page(fixture("portal.html")),
        (Some("/details"), "term=0") => page(showing_previous(fixture("policy_details.html"))),
        (Some("/details"), _) => page(fixture("policy_details.html")),
        (Some("/documents"), _) => page(fixture("documents.html")),
        (Some("/payments"), "billing_status=upcoming") => page(fixture("payments_upcoming.html")),
        (Some("/payments"), "billing_status=approved") => page(fixture("payments_past.html")),
        _ => Reply {
            status: 404,
            headers: Vec::new(),
            body: String::new(),
        },
    }
}

/// One request as it arrived.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    cookie: Option<String>,
    raw: String,
}

type Answer = Arc<dyn Fn(usize, &str) -> Reply + Send + Sync>;

struct Stub {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                handle(stream, &answer, &log);
            }
        });
        Self { base, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn handle(stream: TcpStream, answer: &Answer, log: &Mutex<Vec<Seen>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut raw = line.clone();
    let mut cookie = None;
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
        raw.push_str(&header);
        let lower = header.to_ascii_lowercase();
        if lower.starts_with("cookie:") {
            cookie = Some(header["cookie:".len()..].trim().to_string());
        }
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    raw.push_str(&String::from_utf8_lossy(&body));

    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let index = {
        let mut seen = log.lock().unwrap();
        seen.push(Seen {
            line: format!("{method} {target}"),
            cookie,
            raw,
        });
        seen.len() - 1
    };
    let reply = answer(index, &target);
    let mut response = format!(
        "HTTP/1.1 {} Stub\r\ncontent-length: {}\r\nconnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for header in &reply.headers {
        response.push_str(header);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    response.push_str(&reply.body);
    let mut stream = stream;
    let _ = stream.write_all(response.as_bytes());
}

/// [`MemVault`], counting what the server must not do.
#[derive(Clone, Default)]
struct Vault_ {
    inner: Rc<MemVault>,
    loads: Rc<Cell<u32>>,
    writes: Rc<Cell<u32>>,
    /// What a load fails with, when a test makes it fail.
    broken: Rc<Cell<Option<&'static str>>>,
}

impl Vault for Vault_ {
    fn service(&self) -> &str {
        self.inner.service()
    }
    fn load(&self, account: &str) -> Result<Option<Bundle>, CliError> {
        self.loads.set(self.loads.get() + 1);
        if let Some(message) = self.broken.get() {
            return Err(CliError::Keychain(message.to_string()));
        }
        self.inner.load(account)
    }
    fn save(&self, account: &str, bundle: &Bundle) -> Result<(), CliError> {
        self.writes.set(self.writes.get() + 1);
        self.inner.save(account, bundle)
    }
    fn delete(&self, account: &str) -> Result<bool, CliError> {
        self.writes.set(self.writes.get() + 1);
        self.inner.delete(account)
    }
}

fn cookie_header(value: &str) -> String {
    format!("{SESSION_COOKIE}={value}")
}

fn jar_of(value: &str) -> Jar {
    let mut jar = Jar::new();
    jar.set_cookie(&format!("{}; path=/", cookie_header(value)));
    jar
}

static DIRS: AtomicU32 = AtomicU32::new(0);

fn config_file(contents: &Value) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kin-mcp-readonly-{}-{}",
        std::process::id(),
        DIRS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
    std::fs::write(&path, contents.to_string()).unwrap();
    path
}

/// A signed-in login with a password and session stored, over a stub portal.
fn signed_in(answer: Answer) -> (KinTools<LivePortal<Vault_>>, Stub, Vault_) {
    let vault = Vault_::default();
    vault
        .inner
        .save(
            ACCOUNT,
            &Bundle {
                password: Some(PASSWORD.into()),
                jar: Some(jar_of(STORED)),
                parked: None,
            },
        )
        .unwrap();
    let stub = Stub::start(answer);
    let tools = over(&stub, vault.clone(), &json!({"username": ACCOUNT}));
    (tools, stub, vault)
}

fn over(stub: &Stub, vault: Vault_, config: &Value) -> KinTools<LivePortal<Vault_>> {
    let config = ConfigStore::new("kin").with_override(Some(config_file(config)));
    KinTools::new(LivePortal::new(&stub.base, vault, config))
}

fn every_tool_call() -> Vec<(&'static str, Value)> {
    vec![
        ("kin_policies", json!({})),
        ("kin_policies", json!({"response_format": "detailed"})),
        ("kin_policy", json!({})),
        ("kin_policy", json!({"response_format": "detailed"})),
        ("kin_policy", json!({"term": "previous"})),
        ("kin_documents", json!({})),
        ("kin_documents", json!({"response_format": "detailed"})),
        ("kin_payments", json!({})),
        ("kin_payments", json!({"past": true})),
        ("kin_payments", json!({"all": true, "limit": 100})),
    ]
}

fn text(result: &CallToolResult) -> &str {
    result.text()
}

fn fails_with(result: &CallToolResult, kind: &str) -> String {
    assert!(result.is_error(), "{}", result.text());
    let text = text(result).to_string();
    assert!(text.starts_with(kind), "{text}");
    text
}

/// A tool definition is the whole of what a client can ask for. Every one is marked
/// read-only and takes only arguments that narrow a read.
#[test]
fn no_tool_definition_reaches_a_mutating_command() {
    let (tools, _stub, _vault) = signed_in(Arc::new(|_, target| fixtures(target)));
    let defs = tools.tools();
    assert_eq!(
        defs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        [
            "kin_policies",
            "kin_policy",
            "kin_documents",
            "kin_payments"
        ]
    );
    let readers = [
        "policy",
        "term",
        "past",
        "all",
        "limit",
        "cursor",
        "response_format",
    ];
    for tool in &defs {
        assert!(tool.annotations.read_only, "{}", tool.name);
        assert!(!tool.annotations.destructive, "{}", tool.name);
        assert!(!tool.requires_user_interaction(), "{}", tool.name);
        for arg in tool.input_schema["properties"].as_object().unwrap().keys() {
            assert!(readers.contains(&arg.as_str()), "{}: {arg}", tool.name);
        }
    }
}

#[test]
fn every_tool_answers_with_gets_to_the_read_pages_alone() {
    let (tools, stub, vault) = signed_in(Arc::new(|_, target| fixtures(target)));
    for (name, args) in every_tool_call() {
        let result = call(&tools, name, args.clone());
        assert!(!result.is_error(), "{name} {args}: {}", result.text());
        let got = assert_fits(&tools, name, &result).to_string();
        for held_back in ["active_storage", "http", "authenticity_token", "adyen"] {
            assert!(
                !got.contains(held_back),
                "{name} {args}: {held_back} in {got}"
            );
        }
    }
    let seen = stub.seen();
    assert!(seen.len() >= every_tool_call().len());
    for request in &seen {
        assert!(READS.contains(&request.line.as_str()), "{}", request.line);
        assert!(!request.raw.contains(PASSWORD), "{}", request.line);
        let cookie = request.cookie.as_deref().unwrap_or_default();
        assert!(cookie.contains(SESSION_COOKIE), "{}", request.line);
    }
    assert_eq!(vault.writes.get(), 0);
}

/// Kin re-issues the session cookie on every response. The next read sends the newest,
/// and the keychain stays as it was.
#[test]
fn a_rotated_cookie_is_sent_next_and_the_keychain_is_not_written() {
    let (tools, stub, vault) = signed_in(Arc::new(|index, target| Reply {
        headers: vec![format!(
            "set-cookie: {}; path=/",
            cookie_header(&format!("rotated-{index}"))
        )],
        ..fixtures(target)
    }));
    for _ in 0..3 {
        let result = call(&tools, "kin_policies", json!({}));
        assert!(!result.is_error(), "{}", result.text());
    }
    let cookies: Vec<String> = stub.seen().into_iter().filter_map(|s| s.cookie).collect();
    assert_eq!(
        cookies,
        [
            cookie_header(STORED),
            cookie_header("rotated-0"),
            cookie_header("rotated-1")
        ]
    );
    assert_eq!(vault.writes.get(), 0);
    let stored = vault.inner.load(ACCOUNT).unwrap().unwrap();
    assert_eq!(stored.jar.unwrap(), jar_of(STORED));
}

/// A new sign-in changes what is stored, and takes effect on the next read.
#[test]
fn a_new_sign_in_retires_the_rotated_cookie() {
    let (tools, stub, vault) = signed_in(Arc::new(|index, target| Reply {
        headers: vec![format!(
            "set-cookie: {}; path=/",
            cookie_header(&format!("rotated-{index}"))
        )],
        ..fixtures(target)
    }));
    assert!(!call(&tools, "kin_policies", json!({})).is_error());
    vault
        .inner
        .save(
            ACCOUNT,
            &Bundle {
                jar: Some(jar_of("signed-in-again")),
                ..Bundle::default()
            },
        )
        .unwrap();
    assert!(!call(&tools, "kin_policies", json!({})).is_error());
    let cookies: Vec<String> = stub.seen().into_iter().filter_map(|s| s.cookie).collect();
    assert_eq!(cookies[1], cookie_header("signed-in-again"));
}

/// The lapse a CLI command would answer by signing in, which spends a one-time code.
/// Here it is an error: one GET, no sign-in page, no form, and Kin's anonymous cookie
/// does not replace the session.
#[test]
fn a_lapsed_session_is_an_auth_error_and_nothing_signs_in() {
    let (tools, stub, vault) = signed_in(Arc::new(|_, _| Reply {
        status: 302,
        headers: vec![
            "location: /users/sign_in".into(),
            format!("set-cookie: {}; path=/", cookie_header("anonymous")),
        ],
        body: String::new(),
    }));
    for _ in 0..2 {
        let text = fails_with(&call(&tools, "kin_policies", json!({})), "auth:");
        assert!(text.contains("`kin auth login`"), "{text}");
    }
    let seen = stub.seen();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert_eq!(request.line, "GET /kin_portal");
        assert_eq!(request.cookie, Some(cookie_header(STORED)));
        assert!(!request.raw.contains(PASSWORD));
    }
    assert_eq!(vault.writes.get(), 0);
}

#[test]
fn the_sign_in_page_answered_directly_is_an_auth_error() {
    let (tools, stub, _vault) = signed_in(Arc::new(|_, _| page(fixture("sign_in.html"))));
    fails_with(&call(&tools, "kin_policies", json!({})), "auth:");
    assert_eq!(stub.seen().len(), 1);
}

#[test]
fn no_stored_session_is_an_auth_error_without_a_request() {
    let stub = Stub::start(Arc::new(|_, target| fixtures(target)));
    let vault = Vault_::default();
    vault
        .inner
        .save(
            ACCOUNT,
            &Bundle {
                password: Some(PASSWORD.into()),
                ..Bundle::default()
            },
        )
        .unwrap();
    let tools = over(&stub, vault, &json!({"username": ACCOUNT}));
    fails_with(&call(&tools, "kin_policies", json!({})), "auth:");
    assert!(stub.seen().is_empty());
}

#[test]
fn no_configured_login_is_an_auth_error_before_the_keychain() {
    let stub = Stub::start(Arc::new(|_, target| fixtures(target)));
    let vault = Vault_::default();
    let tools = over(&stub, vault.clone(), &json!({}));
    let text = fails_with(&call(&tools, "kin_policies", json!({})), "auth:");
    assert!(text.contains("`kin auth login`"), "{text}");
    assert_eq!(vault.loads.get(), 0);
    assert!(stub.seen().is_empty());
}

/// A decode error names the account and can quote the item. Neither crosses.
#[test]
fn a_keychain_error_says_nothing_of_the_item_or_the_account() {
    let (tools, stub, vault) = signed_in(Arc::new(|_, target| fixtures(target)));
    vault.broken.set(Some(
        "someone@example.com: invalid type: string \"dummy-fragment\", expected a bundle",
    ));
    let result = call(&tools, "kin_policies", json!({}));
    assert!(result.is_error());
    let text = result.text();
    assert!(text.contains("`kin auth status`"), "{text}");
    for held_back in ["someone", "example.com", "dummy-fragment", "bundle"] {
        assert!(!text.contains(held_back), "{held_back} in {text}");
    }
    assert!(stub.seen().is_empty());
}

/// The configured default policy is read on each call, and is only a query over the
/// login's own list.
#[test]
fn the_configured_policy_is_matched_against_the_list() {
    let stub = Stub::start(Arc::new(|_, target| fixtures(target)));
    let vault = Vault_::default();
    vault
        .inner
        .save(
            ACCOUNT,
            &Bundle {
                jar: Some(jar_of(STORED)),
                ..Bundle::default()
            },
        )
        .unwrap();
    let tools = over(
        &stub,
        vault,
        &json!({"username": ACCOUNT, "policy": "KIN-DP-FL-000000009"}),
    );
    fails_with(&call(&tools, "kin_policy", json!({})), "not_found:");
    assert_eq!(
        stub.seen()
            .iter()
            .map(|s| s.line.as_str())
            .collect::<Vec<_>>(),
        ["GET /kin_portal"]
    );
}

#[test]
fn a_server_error_or_an_unsigned_page_is_an_upstream_error() {
    let replies = [
        Reply {
            status: 500,
            headers: Vec::new(),
            body: String::new(),
        },
        // A 200 with no sign-out form: not a page a signed-in session sees.
        page("<html>Scheduled maintenance</html>".into()),
    ];
    for reply in replies {
        let (tools, _stub, _vault) = signed_in(Arc::new(move |_, _| reply.clone()));
        fails_with(&call(&tools, "kin_policies", json!({})), "upstream:");
    }
}

#[test]
fn a_redirect_off_the_portal_is_followed_nowhere() {
    let (tools, stub, _vault) = signed_in(Arc::new(|_, _| Reply {
        status: 302,
        headers: vec!["location: https://elsewhere.example/phish".into()],
        body: String::new(),
    }));
    fails_with(&call(&tools, "kin_policies", json!({})), "upstream:");
    assert_eq!(stub.seen().len(), 1);
}

/// What a redesign would look like, through the real read path: an error for each
/// tool, not an empty answer.
#[test]
fn a_changed_page_is_an_error_through_the_real_read_path() {
    fn changed(target: &str) -> Reply {
        let reply = fixtures(target);
        Reply {
            body: reply
                .body
                .replace("policy-document__item", "renamed__item")
                .replace("billing-transactions", "renamed-frame")
                .replace("data-rspec", "data-renamed")
                .replace("id=\"deductibles\"", "id=\"renamed\""),
            ..reply
        }
    }
    let (tools, _stub, _vault) = signed_in(Arc::new(|_, target| changed(target)));
    for name in ["kin_policy", "kin_documents", "kin_payments"] {
        fails_with(&call(&tools, name, json!({})), "upstream:");
    }
    let (tools, _stub, _vault) = signed_in(Arc::new(|_, target| {
        let reply = fixtures(target);
        Reply {
            body: reply.body.replace("policy-summary", "renamed"),
            ..reply
        }
    }));
    fails_with(&call(&tools, "kin_policies", json!({})), "upstream:");
}
