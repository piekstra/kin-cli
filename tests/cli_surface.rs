//! Black-box tests of the binary's surface. None of them reaches the
//! keychain or the network: each case either renders help/info or is refused
//! by argument checks that run before any credential read (SPEC §1.5). The
//! config directory is a temp dir, never the developer's.

use assert_cmd::Command;
use predicates::prelude::*;

fn kin(config_home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("kin").unwrap();
    cmd.env("XDG_CONFIG_HOME", config_home)
        .env_remove("KIN_PASSWORD")
        .env_remove("NO_COLOR");
    cmd
}

/// A fresh config dir per call. The counter, not the clock, makes it
/// unique: parallel tests read the same microsecond often enough on macOS,
/// and a shared dir would hand one test another's `username` — and with it
/// a keychain read and a request to Kin.
fn tmp() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kin-cli-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_command_renders_help() {
    let home = tmp();
    for args in [
        vec!["--help"],
        vec!["auth", "--help"],
        vec!["auth", "login", "--help"],
        vec!["auth", "status", "--help"],
        vec!["auth", "logout", "--help"],
        vec!["auth", "set-credential", "--help"],
        vec!["config", "--help"],
        vec!["config", "set", "--help"],
        vec!["policies", "--help"],
        vec!["policies", "list", "--help"],
        vec!["policies", "get", "--help"],
        vec!["documents", "--help"],
        vec!["documents", "list", "--help"],
        vec!["documents", "download", "--help"],
        vec!["payments", "--help"],
        vec!["payments", "list", "--help"],
        vec!["self-update", "--help"],
        vec!["completions", "--help"],
        vec!["info", "--help"],
    ] {
        kin(&home).args(&args).assert().success();
    }
}

#[test]
fn info_describes_the_cli() {
    let home = tmp();
    let out = kin(&home).arg("info").output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "kin");
    assert_eq!(v["spec"], "piekstra-cli/1");
    assert_eq!(v["auth"]["method"], "password");
    assert_eq!(v["profiles"][0], "documents/v1");
    let caps: Vec<&str> = v["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(caps, ["policies", "documents", "payments"]);
}

#[test]
fn no_color_in_the_environment_never_fails_a_command() {
    // cli-common v0.15.1: any non-empty NO_COLOR disables color, per
    // no-color.org; before it, NO_COLOR=1 was a usage error.
    let home = tmp();
    for v in ["1", "true", "0", ""] {
        kin(&home)
            .env("NO_COLOR", v)
            .args(["info"])
            .assert()
            .success();
    }
}

#[test]
fn completions_render() {
    let home = tmp();
    for shell in ["bash", "zsh", "fish"] {
        kin(&home)
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("kin"));
    }
}

#[test]
fn bad_arguments_are_usage_errors() {
    let home = tmp();
    for args in [
        vec!["policies", "get", "--term", "last-year"],
        vec!["documents", "download"],
        vec!["documents", "download", "--all"],
        vec!["documents", "download", "--all", "-o", "-"],
        vec!["documents", "download", "x", "--all", "-o", "d"],
        vec!["documents", "list", "--since", "yesterday"],
        vec!["payments", "list", "--past", "--all"],
        vec!["auth", "login", "--channel", "sms"],
        vec!["auth", "login", "--username", "not-an-email"],
        vec!["auth", "login", "--code", "12"],
        vec!["config", "set", "mailbox", "gmail"],
        vec!["config", "set", "otp_channel", "pigeon"],
        vec!["config", "set", "nonsense", "x"],
    ] {
        kin(&home).args(&args).assert().code(2);
    }
}

#[test]
fn a_usage_error_in_json_mode_is_one_error_document() {
    let home = tmp();
    let out = kin(&home)
        .args(["--json", "documents", "download"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "usage");
    assert!(v["error"]["message"].as_str().unwrap().contains("--all"));
}

#[test]
fn reads_without_a_configured_login_are_auth_errors() {
    // No username configured: exit 3 before any keychain read.
    let home = tmp();
    for args in [
        vec!["policies", "list"],
        vec!["documents", "list"],
        vec!["payments", "list"],
    ] {
        kin(&home)
            .args(&args)
            .assert()
            .code(3)
            .stderr(predicate::str::contains("auth login --username"));
    }
}

#[test]
fn auth_status_works_logged_out() {
    let home = tmp();
    let out = kin(&home)
        .args(["--json", "auth", "status"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["authenticated"], false);
    assert_eq!(v["method"], "password");
}

#[test]
fn config_round_trips_through_a_temp_dir() {
    let home = tmp();
    kin(&home)
        .args(["config", "set", "username", "pat@example.com"])
        .assert()
        .success();
    kin(&home)
        .args(["config", "set", "mailbox", "gro:work"])
        .assert()
        .success();
    let out = kin(&home)
        .args(["--json", "config", "show"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["username"], "pat@example.com");
    assert_eq!(v["mailbox"], "gro:work");
    kin(&home)
        .args(["config", "unset", "mailbox"])
        .assert()
        .success();
    kin(&home)
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(home.to_str().unwrap()));
}
