//! The `kin mcp` tools, offline: `tests/fixtures/` behind [`Reads`], checked by
//! cli-common's conformance helper. `tests/mcp_readonly.rs` puts a stub portal behind
//! the real [`LivePortal`]; `tests/mcp_stdio.rs` and `tests/mcp_http.rs` drive the
//! real binary.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use pk_cli_mcp::conformance::{assert_conforms, assert_fits, call, definitions_size};
use serde_json::json;

use super::*;

const TOOLS: [&str; 4] = [
    "kin_policies",
    "kin_policy",
    "kin_documents",
    "kin_payments",
];
const NUMBER: &str = "KIN-DP-FL-000000001";
const DETAILS: &str = "/kin_portal/policies/KIN-DP-FL-000000001/details";
const DOCUMENTS: &str = "/kin_portal/policies/KIN-DP-FL-000000001/documents";
const UPCOMING: &str = "/kin_portal/policies/KIN-DP-FL-000000001/payments?billing_status=upcoming";
const PAST: &str = "/kin_portal/policies/KIN-DP-FL-000000001/payments?billing_status=approved";

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

/// Each path's page, by default its fixture.
struct Fixtures {
    pages: HashMap<String, String>,
    reads: Rc<RefCell<Vec<String>>>,
    policy_setting: Option<String>,
    failure: Option<fn() -> CliError>,
    config_read: Rc<Cell<u32>>,
}

impl Fixtures {
    fn new() -> Self {
        let pages = [
            (PORTAL, "portal.html"),
            (DETAILS, "policy_details.html"),
            (DOCUMENTS, "documents.html"),
            (UPCOMING, "payments_upcoming.html"),
            (PAST, "payments_past.html"),
        ];
        Self {
            pages: pages
                .into_iter()
                .map(|(path, name)| (path.to_string(), fixture(name)))
                .collect(),
            reads: Rc::default(),
            policy_setting: None,
            failure: None,
            config_read: Rc::default(),
        }
    }

    fn with_page(mut self, path: &str, html: String) -> Self {
        self.pages.insert(path.to_string(), html);
        self
    }

    fn tools(self) -> (KinTools<Self>, Rc<RefCell<Vec<String>>>) {
        let reads = Rc::clone(&self.reads);
        (KinTools::new(self), reads)
    }
}

impl Reads for Fixtures {
    fn page(&self, path: &str) -> Result<String, CliError> {
        self.reads.borrow_mut().push(path.to_string());
        if let Some(failure) = self.failure {
            return Err(failure());
        }
        self.pages
            .get(path)
            .cloned()
            .ok_or_else(|| CliError::NotFound(format!("no page at {path}")))
    }

    fn default_policy(&self) -> Result<Option<String>, CliError> {
        self.config_read.set(self.config_read.get() + 1);
        Ok(self.policy_setting.clone())
    }
}

fn ok<R: Reads>(tools: &KinTools<R>, name: &str, args: Value) -> Value {
    let result = call(tools, name, args);
    assert!(!result.is_error(), "{name}: {}", result.text());
    assert_fits(tools, name, &result).clone()
}

fn err<R: Reads>(tools: &KinTools<R>, name: &str, args: Value) -> String {
    let result = call(tools, name, args);
    assert!(result.is_error(), "{name}: {}", result.text());
    result.text().to_string()
}

/// The portal page with a second card for `number`.
fn portal_with_second_policy(number: &str, status: &str) -> String {
    let html = fixture("portal.html");
    let card = scrape::blocks_with_class(&html, "div", "policy-summary")
        .into_iter()
        .next()
        .unwrap();
    let second = card.replace(NUMBER, number).replace("Active", status);
    html.replacen(&card, &format!("{card}\n{second}"), 1)
}

#[test]
fn the_tools_conform_and_fit_the_budget() {
    let (tools, _) = Fixtures::new().tools();
    assert_conforms(&tools);
    let size = definitions_size(&tools);
    assert!(size < 8000, "{size}");
    assert_eq!(
        tools
            .tools()
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        TOOLS
    );
    for tool in tools.tools() {
        assert!(tool.annotations.read_only, "{}", tool.name);
    }
}

#[test]
fn listing_tools_reads_nothing() {
    let fixtures = Fixtures::new();
    let config_read = Rc::clone(&fixtures.config_read);
    let (tools, reads) = fixtures.tools();
    let _ = tools.tools();
    let _ = tools.server();
    assert!(reads.borrow().is_empty());
    assert_eq!(config_read.get(), 0);
}

#[test]
fn policies_lists_the_cards_and_detailed_adds_kind_and_flood() {
    let (tools, _) = Fixtures::new().tools();
    let got = ok(&tools, "kin_policies", json!({}));
    assert_eq!(got["returned"], 1);
    let item = &got["items"][0];
    assert_eq!(item["number"], NUMBER);
    assert!(
        item["address"].as_str().unwrap().contains("Example"),
        "{item}"
    );
    assert_eq!(item["status"], "Active");
    assert!(item.get("kind").is_none());
    assert!(item.get("flood_endorsement").is_none());

    let got = ok(
        &tools,
        "kin_policies",
        json!({"response_format": "detailed"}),
    );
    assert_eq!(got["items"][0]["kind"], "Homeowner");
    assert!(got["items"][0]["flood_endorsement"].is_boolean());
}

#[test]
fn policy_gives_coverage_as_decimal_strings_and_holds_back_the_rest() {
    let (tools, _) = Fixtures::new().tools();
    let got = ok(&tools, "kin_policy", json!({}));
    assert_eq!(got["number"], NUMBER);
    assert!(
        got["term"]["label"].as_str().unwrap().contains("Current"),
        "{got}"
    );
    assert!(got["current_premium"].is_string(), "{got}");
    assert!(!got["coverages"].as_array().unwrap().is_empty());
    assert!(!got["deductibles"].as_array().unwrap().is_empty());
    assert!(!got["terms"].as_array().unwrap().is_empty());
    let coverage = &got["coverages"][0];
    let amount = coverage["amount"].as_str().unwrap();
    assert!(
        amount.ends_with(".00") && !amount.contains('$'),
        "{coverage}"
    );
    assert!(coverage.get("description").is_none());
    assert!(got.get("loan_number").is_none(), "{got}");
    assert!(got.get("applicants").is_none(), "{got}");
    assert!(got.get("features").is_none(), "{got}");
    assert!(got.get("other").is_none(), "{got}");
    assert!(got["lender"].is_string(), "{got}");
}

#[test]
fn a_detailed_policy_adds_applicants_the_loan_and_features() {
    let (tools, _) = Fixtures::new().tools();
    let got = ok(&tools, "kin_policy", json!({"response_format": "detailed"}));
    assert!(!got["applicants"].as_array().unwrap().is_empty(), "{got}");
    assert!(got["loan_number"].is_string(), "{got}");
    assert!(!got["features"].as_array().unwrap().is_empty(), "{got}");
    assert!(got["coverages"][0].get("description").is_some(), "{got}");
}

#[test]
fn a_page_that_lost_the_coverage_lines_is_an_error_and_not_no_coverage() {
    let html = fixture("policy_details.html")
        .replace("data-rspec", "data-renamed")
        .replace("id=\"deductibles\"", "id=\"renamed\"");
    let (tools, _) = Fixtures::new().with_page(DETAILS, html).tools();
    let text = err(&tools, "kin_policy", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");
    assert!(text.contains("may have changed"), "{text}");
}

#[test]
fn a_details_page_for_another_policy_is_an_error() {
    let html = fixture("policy_details.html").replace(NUMBER, "KIN-DP-FL-000000002");
    let (tools, _) = Fixtures::new().with_page(DETAILS, html).tools();
    let text = err(&tools, "kin_policy", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");
}

#[test]
fn the_previous_term_uses_the_value_on_kins_own_selector() {
    let details = fixture("policy_details.html");
    let terms = parse::terms(&details);
    assert!(terms.len() >= 2, "the fixture needs two terms");
    let previous = terms[1].value.clone();
    let shown = details.replace(" selected>", ">").replace(
        &format!("value=\"{previous}\""),
        &format!("value=\"{previous}\" selected"),
    );
    let path = format!("{DETAILS}?term={previous}");
    let (tools, reads) = Fixtures::new().with_page(&path, shown).tools();
    let got = ok(&tools, "kin_policy", json!({"term": "previous"}));
    assert_eq!(got["term"]["label"], terms[1].label.as_str());
    assert_eq!(reads.borrow().last().unwrap(), &path);
}

/// Kin ignoring `?term=` would show the current term's limits as last year's.
#[test]
fn a_page_still_showing_the_current_term_is_not_the_previous_one() {
    let details = fixture("policy_details.html");
    let previous = parse::terms(&details)[1].value.clone();
    let path = format!("{DETAILS}?term={previous}");
    let (tools, _) = Fixtures::new().with_page(&path, details).tools();
    let text = err(&tools, "kin_policy", json!({"term": "previous"}));
    assert!(text.starts_with("upstream:"), "{text}");
}

#[test]
fn an_unknown_term_is_not_found_and_a_bad_one_is_refused_before_any_read() {
    let (tools, reads) = Fixtures::new().tools();
    let text = err(&tools, "kin_policy", json!({"term": "1999-01-01"}));
    assert!(text.starts_with("not_found:"), "{text}");
    reads.borrow_mut().clear();
    let text = err(&tools, "kin_policy", json!({"term": "sideways"}));
    assert!(text.starts_with("usage:"), "{text}");
    assert!(reads.borrow().is_empty());
}

/// A pre-signed link works without the session, so it is out of every format.
#[test]
fn documents_name_documents_and_carry_no_link() {
    let (tools, _) = Fixtures::new().tools();
    assert!(fixture("documents.html").contains("active_storage"));
    for args in [
        json!({}),
        json!({"response_format": "detailed", "limit": 100}),
    ] {
        let got = ok(&tools, "kin_documents", args.clone());
        assert_eq!(got["policy"], NUMBER);
        assert!(got["returned"].as_u64().unwrap() > 0);
        assert!(got["items"][0]["id"].is_string());
        let text = got.to_string();
        for held_back in ["active_storage", "http", "/rails/", "signature"] {
            assert!(!text.contains(held_back), "{args}: {held_back} in {text}");
        }
    }
    let got = ok(&tools, "kin_documents", json!({}));
    assert!(got["items"][0].get("file").is_none());
    let got = ok(
        &tools,
        "kin_documents",
        json!({"response_format": "detailed"}),
    );
    assert!(got["items"][0]["file"].is_string());
}

#[test]
fn an_empty_or_partly_read_documents_page_is_an_error() {
    let renamed = fixture("documents.html").replace("policy-document__item", "renamed__item");
    let (tools, _) = Fixtures::new().with_page(DOCUMENTS, renamed).tools();
    let text = err(&tools, "kin_documents", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");

    // One row whose link lost the hook is a document the list would drop.
    let html = fixture("documents.html");
    let unreadable = html.replacen("data-document-label", "data-renamed", 1);
    let (tools, _) = Fixtures::new().with_page(DOCUMENTS, unreadable).tools();
    let text = err(&tools, "kin_documents", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");
}

#[test]
fn payments_default_to_upcoming_and_past_and_all_read_what_they_say() {
    let (tools, reads) = Fixtures::new().tools();
    let got = ok(&tools, "kin_payments", json!({}));
    assert_eq!(reads.borrow().last().unwrap(), UPCOMING);
    assert_eq!(got["policy"], NUMBER);
    let upcoming = got["returned"].as_u64().unwrap();
    assert!(upcoming > 0);
    assert_eq!(got["items"][0]["status"], "scheduled");
    assert!(got["items"][0]["date"].is_string());

    reads.borrow_mut().clear();
    let got = ok(&tools, "kin_payments", json!({"past": true, "limit": 100}));
    assert_eq!(reads.borrow().as_slice(), [PORTAL, PAST]);
    let past = got["returned"].as_u64().unwrap();
    assert!(past > 0);

    reads.borrow_mut().clear();
    let got = ok(&tools, "kin_payments", json!({"all": true, "limit": 100}));
    assert_eq!(reads.borrow().as_slice(), [PORTAL, UPCOMING, PAST]);
    assert_eq!(got["returned"].as_u64().unwrap(), upcoming + past);
}

#[test]
fn past_and_all_together_are_refused_before_any_read() {
    let (tools, reads) = Fixtures::new().tools();
    let text = err(&tools, "kin_payments", json!({"past": true, "all": true}));
    assert!(text.starts_with("usage:"), "{text}");
    assert!(reads.borrow().is_empty());
}

#[test]
fn a_payment_amount_is_a_decimal_string_and_the_card_waits_for_detailed() {
    let (tools, _) = Fixtures::new().tools();
    let got = ok(&tools, "kin_payments", json!({}));
    let item = &got["items"][0];
    assert_eq!(item["amount"], "111.00");
    assert!(item.get("card_last4").is_none(), "{item}");
    let got = ok(
        &tools,
        "kin_payments",
        json!({"response_format": "detailed"}),
    );
    assert_eq!(got["items"][0]["card_last4"], "0000");
}

/// The upcoming page embeds a live payment form. Nothing of it is read or returned.
#[test]
fn nothing_of_the_payment_form_reaches_a_result() {
    assert!(fixture("payments_upcoming.html").contains("make-payment-submit"));
    let (tools, _) = Fixtures::new().tools();
    let text = ok(
        &tools,
        "kin_payments",
        json!({"response_format": "detailed"}),
    )
    .to_string();
    for held_back in ["authenticity_token", "adyen", "make-payment", "form"] {
        assert!(!text.contains(held_back), "{held_back} in {text}");
    }
}

#[test]
fn a_page_that_lost_the_payments_is_an_error_and_not_nothing_due() {
    let cases = [
        (
            "no frame",
            fixture("payments_upcoming.html").replace("billing-transactions", "renamed"),
        ),
        (
            "rows renamed",
            fixture("payments_upcoming.html").replace("billing-transaction__item", "renamed__item"),
        ),
        (
            "row with no amount",
            fixture("payments_upcoming.html")
                .replace("m-l-auto", "renamed")
                .replace("billing-transaction-amount", "renamed"),
        ),
        (
            "amount in another shape",
            fixture("payments_upcoming.html").replace(">$111<", ">one hundred<"),
        ),
    ];
    for (case, html) in cases {
        let (tools, _) = Fixtures::new().with_page(UPCOMING, html).tools();
        let text = err(&tools, "kin_payments", json!({}));
        assert!(text.starts_with("upstream:"), "{case}: {text}");
    }
}

#[test]
fn an_empty_list_in_the_frame_is_nothing_due() {
    let html = r#"<form action="/users/sign_out"></form>
        <turbo-frame id="billing-transactions"><p>No upcoming payments.</p></turbo-frame>"#;
    let (tools, _) = Fixtures::new()
        .with_page(UPCOMING, html.to_string())
        .tools();
    let got = ok(&tools, "kin_payments", json!({}));
    assert_eq!(got["returned"], 0);
}

#[test]
fn payments_page_with_a_cursor_bound_to_the_view() {
    let (tools, _) = Fixtures::new().tools();
    let first = ok(&tools, "kin_payments", json!({"past": true, "limit": 2}));
    assert_eq!(first["returned"], 2);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let second = ok(
        &tools,
        "kin_payments",
        json!({"past": true, "limit": 2, "cursor": cursor}),
    );
    assert_ne!(first["items"], second["items"]);
    let text = err(
        &tools,
        "kin_payments",
        json!({"limit": 2, "cursor": cursor}),
    );
    assert!(text.starts_with("usage:"), "{text}");
}

#[test]
fn a_page_that_lost_the_policy_cards_is_an_error_and_not_no_policies() {
    let html = fixture("portal.html").replace("policy-summary", "renamed");
    let (tools, _) = Fixtures::new().with_page(PORTAL, html).tools();
    let text = err(&tools, "kin_policies", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");
}

#[test]
fn a_card_that_cannot_be_read_is_an_error_and_not_one_policy_fewer() {
    let unreadable = portal_with_second_policy("KIN-DP-FL-000000002", "Active").replacen(
        "policy-summary__policy-number",
        "renamed",
        1,
    );
    let (tools, _) = Fixtures::new().with_page(PORTAL, unreadable).tools();
    let text = err(&tools, "kin_policies", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");
}

#[test]
fn a_number_that_is_not_safe_in_a_path_is_an_error() {
    let html = fixture("portal.html").replace(NUMBER, "../users/sign_out");
    let (tools, reads) = Fixtures::new().with_page(PORTAL, html).tools();
    let text = err(&tools, "kin_policy", json!({}));
    assert!(text.starts_with("upstream:"), "{text}");
    assert_eq!(reads.borrow().as_slice(), [PORTAL]);
}

/// A number a tool was handed is only a query over the login's own list, so no
/// argument reaches another customer's policy, or any other page.
#[test]
fn an_argument_never_becomes_a_path() {
    let (tools, reads) = Fixtures::new().tools();
    for policy in [
        "KIN-DP-FL-000000009",
        "../../users/sign_out",
        "/kin_portal/policies/KIN-DP-FL-000000009",
    ] {
        for name in ["kin_policy", "kin_documents", "kin_payments"] {
            reads.borrow_mut().clear();
            let text = err(&tools, name, json!({"policy": policy}));
            assert!(text.starts_with("not_found:"), "{name} {policy}: {text}");
            assert!(text.contains("kin_policies"), "{text}");
            assert_eq!(reads.borrow().as_slice(), [PORTAL], "{name} {policy}");
        }
    }
}

#[test]
fn a_policy_is_found_by_its_number_or_part_of_its_address() {
    let (tools, _) = Fixtures::new().tools();
    for query in ["kin-dp-fl-000000001", "Example"] {
        let got = ok(&tools, "kin_policy", json!({"policy": query}));
        assert_eq!(got["number"], NUMBER, "{query}");
    }
}

#[test]
fn several_policies_ask_which_unless_one_is_active_or_set() {
    let second = "KIN-DP-FL-000000002";
    let two_active = portal_with_second_policy(second, "Active");
    let (tools, _) = Fixtures::new()
        .with_page(PORTAL, two_active.clone())
        .tools();
    let text = err(&tools, "kin_documents", json!({}));
    assert!(text.starts_with("usage:"), "{text}");
    assert!(text.contains(second) && text.contains(NUMBER), "{text}");

    let one_active = portal_with_second_policy(second, "Cancelled");
    let (tools, _) = Fixtures::new().with_page(PORTAL, one_active).tools();
    let got = ok(&tools, "kin_documents", json!({}));
    assert_eq!(got["policy"], NUMBER);

    let mut set = Fixtures::new().with_page(PORTAL, two_active);
    set.policy_setting = Some(NUMBER.to_string());
    let (tools, _) = set.tools();
    let got = ok(&tools, "kin_documents", json!({}));
    assert_eq!(got["policy"], NUMBER);
}

#[test]
fn an_auth_failure_names_the_sign_in_command() {
    let mut fixtures = Fixtures::new();
    fixtures.failure = Some(|| CliError::Auth("the Kin session has lapsed".into()));
    let (tools, _) = fixtures.tools();
    for name in TOOLS {
        let text = err(&tools, name, json!({}));
        assert!(text.starts_with("auth:"), "{name}: {text}");
        assert!(text.contains("kin auth login"), "{name}: {text}");
    }
}

#[test]
fn a_misspelled_argument_is_refused() {
    let (tools, reads) = Fixtures::new().tools();
    let text = err(&tools, "kin_payments", json!({"pst": true}));
    assert!(text.contains("pst"), "{text}");
    assert!(reads.borrow().is_empty());
}

fn jar(value: &str) -> Jar {
    let mut jar = Jar::new();
    jar.set_cookie(&format!("{}={value}; path=/", session_cookie_name()));
    jar
}

fn session_cookie_name() -> &'static str {
    crate::jar::SESSION_COOKIE
}

#[test]
fn a_rotated_jar_is_sent_until_the_stored_one_changes() {
    let mut rotated = Rotated::default();
    let (stored, newer) = (jar("stored"), jar("newer"));
    assert!(rotated.current("a", &stored) == stored);
    rotated.record("a", &stored, &stored, newer.clone());
    assert!(rotated.current("a", &stored) == newer);

    // A new sign-in, or another account, retires the copy.
    assert!(rotated.current("a", &jar("signed-in")) == jar("signed-in"));
    assert!(rotated.current("b", &stored) == stored);
}

#[test]
fn a_slower_read_does_not_put_an_older_jar_back() {
    let mut rotated = Rotated::default();
    let (stored, newer, slower) = (jar("stored"), jar("newer"), jar("slower"));
    rotated.record("a", &stored, &stored, newer.clone());
    // This read went out with the stored jar, and finished after `newer` arrived.
    rotated.record("a", &stored, &stored, slower);
    assert!(rotated.current("a", &stored) == newer);
}
