//! Contract tests over scrubbed captures of the portal's pages
//! (`tests/fixtures/README.md`). Each asserts the fields the commands read,
//! so a Kin redesign fails here loudly instead of emptying a column.

use kin::{dto, parse};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn the_portal_lists_its_policy() {
    let html = fixture("portal.html");
    assert!(parse::is_signed_in(&html));
    let policies = parse::policies(&html);
    assert_eq!(policies.len(), 1);
    let p = &policies[0];
    assert_eq!(p.number, "KIN-DP-FL-000000001");
    assert_eq!(p.address, "100 Example St, Exampleton, FL 00000-0000");
    assert_eq!(p.status.as_deref(), Some("Active"));
    assert_eq!(p.kind.as_deref(), Some("Homeowner"));
    assert!(p.flood, "the flood badge is on the card");
}

#[test]
fn the_details_page_yields_header_terms_and_deductibles() {
    let d = parse::policy_detail(&fixture("policy_details.html")).expect("a details page");
    assert_eq!(d.number, "KIN-DP-FL-000000001");
    assert_eq!(d.status.as_deref(), Some("Active"));
    assert!(d.flood);
    let labels: Vec<&str> = d.header.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Current premium",
            "Policy period",
            "Primary applicant",
            "Co-applicant"
        ]
    );
    assert_eq!(d.header[2].1, "Pat Example");

    assert_eq!(d.terms.len(), 2);
    assert!(d.terms[0].selected && !d.terms[1].selected);
    assert_eq!(d.terms[0].value, "1");
    assert_eq!(d.terms[0].start.as_deref(), Some("2026-01-01"));
    assert_eq!(d.terms[0].end.as_deref(), Some("2027-01-01"));

    let keys: Vec<&str> = d.deductibles.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "standard-deductible",
            "hurricane-deductible",
            "flood-premium"
        ]
    );
    let hurricane = &d.deductibles[1];
    assert_eq!(hurricane.name, "Hurricane");
    assert_eq!(hurricane.basis.as_deref(), Some("(2% of Coverage A)"));
    assert!(hurricane.value.starts_with('$'));
    assert_eq!(
        hurricane.detail.as_deref(),
        Some("Applies to claims for damage caused by hurricanes")
    );
}

#[test]
fn the_details_page_yields_coverages_features_and_lender() {
    let d = parse::policy_detail(&fixture("policy_details.html")).unwrap();
    let cov: Vec<(&str, &str)> = d
        .coverages
        .iter()
        .map(|l| (l.key.as_str(), l.name.as_str()))
        .collect();
    assert_eq!(
        cov,
        [
            ("coverage_a", "Main Structure/Dwelling"),
            ("coverage_b", "Other Structures"),
            ("coverage_c", "Personal Property"),
            ("coverage_d", "Loss of Use"),
            ("coverage_l", "Personal Liability"),
            ("coverage_m", "Medical Payments"),
        ]
    );
    assert!(d.coverages.iter().all(|l| l.value.starts_with('$')));
    assert!(d.coverages[0].description.is_some());

    let flood = d
        .features
        .iter()
        .find(|l| l.key == "flood_and_water_backup")
        .expect("the flood feature line");
    assert_eq!(
        (flood.name.as_str(), flood.value.as_str()),
        ("Flood", "Applied")
    );
    assert!(d.features.len() >= 10, "{} features", d.features.len());

    assert_eq!(
        d.discounts,
        [
            "Electronic Policy",
            "Home Buyer Discount",
            "Windstorm Mitigation"
        ]
    );
    assert!(d.discount_total.is_some());

    let lender = d.lender.expect("the lender block");
    assert_eq!(lender.name, "Example Mortgage Co");
    assert_eq!(lender.loan_number.as_deref(), Some("0000000000"));
    let fields: Vec<&str> = lender.fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(fields, ["Mortgagee clause", "Fax number", "Email"]);
    assert_eq!(
        lender.fields[0].1,
        "ISAOA/ATIMA, P.O. Box 0000, Exampleton, ZZ 00000-0000"
    );
}

#[test]
fn the_policy_dto_carries_money_dates_and_sections() {
    let d = parse::policy_detail(&fixture("policy_details.html")).unwrap();
    let selected = d.terms.iter().find(|t| t.selected).cloned();
    let v = dto::policy_detail(&d, selected.as_ref());
    assert_eq!(v["current_premium"]["currency"], "USD");
    assert_eq!(v["current_period"]["start"], "2026-01-01");
    assert_eq!(v["current_period"]["end"], "2027-01-01");
    assert_eq!(v["flood_premium"]["currency"], "USD");
    let keys: Vec<&str> = v["deductibles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["standard-deductible", "hurricane-deductible"]);
    assert_eq!(v["applicants"][0]["role"], "primary_applicant");
    assert_eq!(v["coverages"][0]["key"], "coverage_a");
    assert!(v["coverages"][0]["limit"]["amount"].is_string());
    assert_eq!(v["deductibles"][1]["basis"], "(2% of Coverage A)");
    assert_eq!(v["flood_endorsement"], true);
    assert_eq!(v["lender"]["loan_number"], "0000000000");
    assert_eq!(
        v["lender"]["mortgagee_clause"]
            .as_str()
            .map(|s| s.starts_with("ISAOA")),
        Some(true)
    );
    assert_eq!(v["terms"].as_array().unwrap().len(), 2);
}

#[test]
fn the_documents_page_lists_downloadable_files() {
    let html = fixture("documents.html");
    let docs = parse::documents(&html);
    let labels: Vec<&str> = docs.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "policy_renewal_packet",
            "declaration_page",
            "amended_subscriber_agreement",
            "notice_of_renewal",
            "notice_of_policy_change"
        ]
    );
    assert_eq!(docs[1].name, "Declaration page");
    assert_eq!(docs[1].file.as_deref(), Some("declaration_page.pdf"));
    assert_eq!(
        docs[0].file.as_deref(),
        Some("KIN-DP-FL-000000001 Renewal Packet for Primary Insured.pdf")
    );
    assert!(docs.iter().all(|d| d
        .url
        .starts_with("https://app.kin.com/rails/active_storage/")));
    assert!(
        docs[2].description.is_none(),
        "an empty description is omitted"
    );

    let terms = parse::terms(&html);
    assert_eq!(terms[0].value, "current_term");
    assert!(terms[0].selected);
    assert_eq!(
        terms[0].start.as_deref(),
        Some("2026-01-01"),
        "the undated current term starts where the previous one ends"
    );
    assert_eq!(terms[1].start.as_deref(), Some("2025-01-01"));
}

#[test]
fn the_upcoming_payment_has_date_amount_and_card() {
    let items = parse::payments(&fixture("payments_upcoming.html"));
    assert_eq!(items.len(), 1);
    let p = &items[0];
    assert_eq!(p.description, "Policy payment 4 of 4");
    assert!(p.amount.starts_with('$'));
    assert_eq!(p.status.as_deref(), Some("scheduled"));
    assert_eq!(p.date.as_deref(), Some("2026-12-01"));
    assert_eq!(p.card_last4.as_deref(), Some("0000"));
}

#[test]
fn past_payments_include_paid_and_issued_refunds() {
    let items = parse::payments(&fixture("payments_past.html"));
    assert_eq!(items.len(), 11);
    assert!(items.iter().all(|p| p.date.is_some()));
    assert!(items.iter().any(|p| p.status.as_deref() == Some("issued")));
    assert!(items.iter().any(|p| p.status.as_deref() == Some("paid")));
    assert!(items.iter().all(|p| p.card_last4.is_none()));
    let v = dto::payment(&items[0]);
    assert_eq!(v["amount"]["currency"], "USD");
}

#[test]
fn the_sign_in_page_offers_a_tokened_form() {
    let html = fixture("sign_in.html");
    assert!(parse::is_login_page(&html));
    assert!(!parse::is_signed_in(&html));
    assert_eq!(
        parse::form_token(&html, "/users/sign_in").as_deref(),
        Some("dummy-csrf-token")
    );
}

#[test]
fn signed_in_pages_are_not_login_pages() {
    for f in [
        "portal.html",
        "policy_details.html",
        "documents.html",
        "payments_past.html",
    ] {
        let html = fixture(f);
        assert!(parse::is_signed_in(&html), "{f}");
        assert!(!parse::is_login_page(&html), "{f}");
    }
}
