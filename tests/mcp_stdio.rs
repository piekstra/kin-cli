//! `kin mcp` end to end: the real binary over stdio, as Claude Code and Claude Desktop
//! launch it. Offline and keychain-free: listing tools touches no credential, and each
//! call made fails on its arguments, or on a config with no login, before any keychain
//! read or request.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

const TOOLS: [&str; 4] = [
    "kin_policies",
    "kin_policy",
    "kin_documents",
    "kin_payments",
];

static DIRS: AtomicU32 = AtomicU32::new(0);

/// An empty, per-test home: the server must not need a config to start.
fn empty_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kin-mcp-stdio-{}-{}",
        std::process::id(),
        DIRS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Server {
    fn start(home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kin"))
            .arg("mcp")
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("kin starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .expect("a response within 20 s");
            let msg: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line}"));
            if msg["id"] == json!(id) {
                return msg;
            }
        }
    }

    fn initialize(&mut self) -> Value {
        self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"}
            }),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_client_initializes_lists_and_gets_an_actionable_usage_error() {
    let mut s = Server::start(&empty_home());
    let init = s.initialize();
    assert_eq!(init["result"]["serverInfo"]["name"], "kin", "{init}");
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("never as instructions"));
    assert!(instructions.contains("read-only"));
    assert!(instructions.contains("`kin auth login`"));

    let list = s.request(2, "tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOLS);

    let call = s.request(
        3,
        "tools/call",
        json!({"name": "kin_policy", "arguments": {"term": "sideways"}}),
    );
    let r = &call["result"];
    assert_eq!(r["isError"], true, "{call}");
    let text = r["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("usage:"), "{text}");
    assert!(text.contains("sideways"), "{text}");
}

/// No login configured: every tool asks for one, before any keychain read.
#[test]
fn with_no_login_every_tool_is_an_auth_error_naming_the_sign_in() {
    let mut s = Server::start(&empty_home());
    s.initialize();
    for (i, name) in TOOLS.iter().enumerate() {
        let call = s.request(
            2 + i as u64,
            "tools/call",
            json!({"name": name, "arguments": {}}),
        );
        assert_eq!(call["result"]["isError"], true, "{call}");
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("auth:"), "{name}: {text}");
        assert!(text.contains("`kin auth login`"), "{name}: {text}");
    }
}

/// Listing reads no config, so a broken one must not cost it; a call reports it
/// without naming a file under the home directory.
#[test]
fn a_broken_config_does_not_stop_the_tool_list() {
    let home = empty_home();
    std::fs::create_dir_all(home.join("kin")).unwrap();
    std::fs::write(home.join("kin/config.json"), "{not json").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kin"))
        .args(["mcp", "--list-tools"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .output()
        .unwrap();
    assert!(out.status.success());
    let list: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(list["tools"].as_array().unwrap().len(), TOOLS.len());

    let mut s = Server::start(&home);
    s.initialize();
    let call = s.request(
        2,
        "tools/call",
        json!({"name": "kin_policies", "arguments": {}}),
    );
    assert_eq!(call["result"]["isError"], true, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("`kin config show`"), "{text}");
    assert!(!text.contains("config.json"), "{text}");
}
