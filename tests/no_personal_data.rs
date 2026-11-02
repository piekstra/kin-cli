//! The repository carries no personal data; runtime output may.
//!
//! Walks every tracked file (`git ls-files`) and looks for the *shapes*
//! personal data takes — phone numbers, street addresses, email addresses,
//! policy and loan numbers, card digits — rather than a denylist of real
//! values, which would publish exactly what it exists to keep out. Dummy
//! values follow obvious patterns (555 numbers, `example.com`, all-zero ids).

use std::process::Command;

fn tracked_files() -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git ls-files");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn digits_run(s: &str, min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else {
            if cur.len() >= min {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    out
}

/// A digit run is a dummy when it is all one digit, zeros then one digit
/// (`000000001`), or a 555 phone number.
fn is_dummy_number(n: &str) -> bool {
    let first = n.chars().next().unwrap();
    let head = &n[..n.len() - 1];
    n.chars().all(|c| c == first)
        || head.chars().all(|c| c == '0')
        || n.starts_with("555")
        || n.starts_with("1555")
}

/// The personal-data shapes on one line, each as `kind value`.
fn scan_line(line: &str) -> Vec<String> {
    let mut findings = Vec::new();
    // Phone, loan, policy and card numbers: 7+ digit runs, and the
    // 3-3-4 phone shape.
    let phoneish = line
        .replace(['(', ')', '-', '.', ' '], "")
        .chars()
        .collect::<String>();
    for n in digits_run(line, 7).into_iter().chain(
        digits_run(&phoneish, 10)
            .into_iter()
            .filter(|n| n.len() <= 11),
    ) {
        if !is_dummy_number(&n) {
            findings.push(format!("number {n}"));
        }
    }
    // Email addresses outside the dummy and Kin's public domains.
    for word in line.split(|c: char| c.is_whitespace() || "\"'<>(),;".contains(c)) {
        if let Some((local, domain)) = word.split_once('@') {
            let domain =
                domain.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
            let ok = local.is_empty()
                || !domain.contains('.')
                || domain == "example.com"
                || domain.ends_with(".example")
                || domain == "kin.com"
                || domain == "help.kin.com"
                || domain.ends_with("users.noreply.github.com");
            if !ok {
                findings.push(format!("email {word}"));
            }
        }
    }
    // Street addresses: a house number followed by a direction or a
    // street suffix, unless it is the dummy street.
    let lower = line.to_ascii_lowercase();
    for suffix in [" way", " st", " ave", " blvd", " dr", " ln", " ct", " rd"] {
        if let Some(pos) = lower.find(suffix) {
            let head = &lower[..pos];
            let has_number = head
                .split_whitespace()
                .rev()
                .take(4)
                .any(|w| w.len() >= 3 && w.chars().all(|c| c.is_ascii_digit()));
            if has_number && !lower.contains("example st") {
                findings.push(format!("address-like `{}`", line.trim()));
            }
        }
    }
    findings
}

#[test]
fn tracked_files_carry_no_personal_data() {
    let files = tracked_files();
    if files.is_empty() {
        // Not a git checkout (a packaged crate): nothing to scan.
        return;
    }
    let mut findings = Vec::new();
    for file in files {
        // This file holds the scanner's own synthetic positives.
        if file.ends_with(".lock")
            || file.starts_with("LICENSE")
            || file == "tests/no_personal_data.rs"
        {
            continue;
        }
        let path = format!("{}/{file}", env!("CARGO_MANIFEST_DIR"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for f in scan_line(line) {
                findings.push(format!("{file}:{}: {f}", i + 1));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "personal-data shapes in tracked files:\n{}",
        findings.join("\n")
    );
}

#[test]
fn identity_fields_in_fixtures_use_dummy_values() {
    // Positive check: every policy number in a fixture is the dummy one.
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let html = std::fs::read_to_string(&path).unwrap();
        let mut rest = html.as_str();
        while let Some(i) = rest.find("KIN-") {
            let number: String = rest[i..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            assert_eq!(number, "KIN-DP-FL-000000001", "{}", path.display());
            rest = &rest[i + 4..];
        }
        // People on a policy appear as applicants; only the dummies may.
        for label in ["Primary applicant:</span>", "Co-applicant:</span>"] {
            if let Some(i) = html.find(label) {
                let after = &html[i + label.len()..];
                let name = after[..after.find('<').unwrap_or(after.len())].trim();
                assert!(
                    name == "Pat Example" || name == "Sam Example",
                    "{}: applicant `{name}` is not a dummy",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn the_scanner_flags_real_shapes_and_passes_dummies() {
    for real in [
        "call (772) 404-1234",
        "loan 8123456789",
        "mail someone@gmail.com",
        "lives at 1234 NW Oak St, Somewhere",
        "member id 7342918",
    ] {
        assert!(!scan_line(real).is_empty(), "missed: {real}");
    }
    for dummy in [
        "call (555) 010-0001",
        "KIN-DP-FL-000000001",
        "pat@example.com, uwsupport@kin.com",
        "100 Example St, Exampleton",
        "$111,111.11",
    ] {
        assert!(
            scan_line(dummy).is_empty(),
            "false positive: {dummy}: {:?}",
            scan_line(dummy)
        );
    }
}
