//! The JSON shapes `kin` emits (SPEC §1.4): snake_case, ISO dates, money as
//! `{"amount": "1234.56", "currency": "USD"}`, unknown fields omitted.

use pk_cli_core::{output, Money};
use serde_json::{json, Map, Value};

use crate::parse::{Deductible, Line, PaymentItem, PolicyDetail, PolicySummary, Term};

/// A `$1,234` value as Money; anything else (`Applied`, `Full`) is `None`.
pub fn money(text: &str) -> Option<Value> {
    let t = text.trim();
    if !t.starts_with('$') && !t.starts_with("-$") {
        return None;
    }
    Money::parse_usd(t).and_then(|m| serde_json::to_value(m).ok())
}

fn put(obj: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(v) = v {
        obj.insert(key.to_string(), v);
    }
}

fn s(v: &Option<String>) -> Option<Value> {
    v.as_ref().map(|x| Value::String(x.clone()))
}

/// `Primary applicant` → `primary_applicant`.
fn snake(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

pub fn policy_summary(p: &PolicySummary) -> Value {
    let mut o = Map::new();
    o.insert("number".into(), json!(p.number));
    put(&mut o, "status", s(&p.status));
    put(&mut o, "kind", s(&p.kind));
    o.insert("address".into(), json!(p.address));
    o.insert("flood_endorsement".into(), json!(p.flood));
    Value::Object(o)
}

fn term(t: &Term) -> Value {
    let mut o = Map::new();
    o.insert("label".into(), json!(t.label));
    put(&mut o, "start", s(&t.start));
    put(&mut o, "end", s(&t.end));
    Value::Object(o)
}

fn deductible(d: &Deductible) -> Value {
    let mut o = Map::new();
    o.insert("key".into(), json!(d.key));
    o.insert("name".into(), json!(d.name));
    put(&mut o, "amount", money(&d.value));
    o.insert("value".into(), json!(d.value));
    put(&mut o, "basis", s(&d.basis));
    put(&mut o, "detail", s(&d.detail));
    Value::Object(o)
}

fn line(l: &Line, money_key: &str) -> Value {
    let mut o = Map::new();
    o.insert("key".into(), json!(l.key));
    o.insert("name".into(), json!(l.name));
    put(&mut o, money_key, money(&l.value));
    o.insert("value".into(), json!(l.value));
    put(&mut o, "description", s(&l.description));
    Value::Object(o)
}

/// `policy/v1` (the schema tag is added by `output::emit`).
pub fn policy_detail(d: &PolicyDetail, selected: Option<&Term>) -> Value {
    let mut o = Map::new();
    o.insert("number".into(), json!(d.number));
    put(&mut o, "status", s(&d.status));
    put(&mut o, "kind", s(&d.kind));
    o.insert("address".into(), json!(d.address));
    o.insert("flood_endorsement".into(), json!(d.flood));
    put(&mut o, "term", selected.map(term));

    let mut applicants = Vec::new();
    let mut other = Vec::new();
    for (label, value) in &d.header {
        match label.as_str() {
            // The header describes the policy as it stands today, whatever
            // term the page is showing: hence `current_*`.
            "Current premium" => put(&mut o, "current_premium", money(value)),
            "Policy period" => {
                if let Some((a, b)) = value.split_once(" - ") {
                    let mut p = Map::new();
                    put(
                        &mut p,
                        "start",
                        crate::parse::parse_month_date(a).map(Value::String),
                    );
                    put(
                        &mut p,
                        "end",
                        crate::parse::parse_month_date(b).map(Value::String),
                    );
                    o.insert("current_period".into(), Value::Object(p));
                }
            }
            l if l.to_ascii_lowercase().contains("applicant") => {
                applicants.push(json!({ "role": snake(l), "name": value }));
            }
            l => other.push(json!({ "label": l, "value": value })),
        }
    }
    if !applicants.is_empty() {
        o.insert("applicants".into(), Value::Array(applicants));
    }
    // Kin lists the flood endorsement's premium in the deductibles card; it
    // is a premium for the shown term, not a deductible.
    let (premiums, deductibles): (Vec<_>, Vec<_>) =
        d.deductibles.iter().partition(|x| x.key == "flood-premium");
    if let Some(fp) = premiums.first() {
        put(&mut o, "flood_premium", money(&fp.value));
    }
    o.insert(
        "deductibles".into(),
        Value::Array(deductibles.into_iter().map(deductible).collect()),
    );
    o.insert(
        "coverages".into(),
        Value::Array(d.coverages.iter().map(|l| line(l, "limit")).collect()),
    );
    o.insert(
        "features".into(),
        Value::Array(d.features.iter().map(|l| line(l, "limit")).collect()),
    );
    if d.discount_total.is_some() || !d.discounts.is_empty() {
        let mut disc = Map::new();
        put(
            &mut disc,
            "total",
            d.discount_total.as_deref().and_then(money),
        );
        disc.insert("items".into(), json!(d.discounts));
        o.insert("discounts".into(), Value::Object(disc));
    }
    if let Some(l) = &d.lender {
        let mut lo = Map::new();
        lo.insert("name".into(), json!(l.name));
        put(&mut lo, "loan_number", s(&l.loan_number));
        for (k, v) in &l.fields {
            lo.insert(snake(k), json!(v));
        }
        o.insert("lender".into(), Value::Object(lo));
    }
    if !other.is_empty() {
        o.insert("other".into(), Value::Array(other));
    }
    o.insert(
        "terms".into(),
        Value::Array(
            d.terms
                .iter()
                .map(|t| {
                    let mut v = term(t);
                    v["selected"] = json!(t.selected);
                    v
                })
                .collect(),
        ),
    );
    Value::Object(o)
}

/// Text view of `policy/v1`: a key/value header, then one table per section.
pub fn render_policy(v: &Value) {
    let mut head = Map::new();
    for key in ["number", "status", "kind", "address", "flood_endorsement"] {
        if let Some(x) = v.get(key) {
            head.insert(key.into(), x.clone());
        }
    }
    if let Some(t) = v.get("term").and_then(|t| t.get("label")) {
        head.insert("term".into(), t.clone());
    }
    if let Some(p) = v.get("flood_premium") {
        head.insert("flood_premium".into(), json!(money_text(p)));
    }
    if let Some(p) = v.get("current_premium") {
        head.insert("current_premium".into(), json!(money_text(p)));
    }
    if let Some(p) = v.get("current_period") {
        head.insert(
            "current_period".into(),
            json!(format!(
                "{} to {}",
                p["start"].as_str().unwrap_or("?"),
                p["end"].as_str().unwrap_or("?")
            )),
        );
    }
    if let Some(a) = v.get("applicants").and_then(Value::as_array) {
        for x in a {
            head.insert(
                x["role"].as_str().unwrap_or("applicant").into(),
                x["name"].clone(),
            );
        }
    }
    output::kv(&Value::Object(head), 0);
    for (title, key) in [
        ("Deductibles", "deductibles"),
        ("Coverage limits", "coverages"),
        ("Other features", "features"),
    ] {
        let rows = output::rows_of(v, key);
        if rows.is_empty() {
            continue;
        }
        println!("\n{title}");
        output::table(&output::table_view(&rows, &["name", "value"]));
    }
    if let Some(l) = v.get("lender") {
        println!("\nLender");
        output::kv(l, 0);
    }
}

fn money_text(v: &Value) -> String {
    serde_json::from_value::<Money>(v.clone())
        .map(|m| m.grouped())
        .unwrap_or_default()
}

pub fn payment(p: &PaymentItem) -> Value {
    let mut o = Map::new();
    put(&mut o, "date", s(&p.date));
    o.insert("description".into(), json!(p.description));
    put(&mut o, "amount", money(&p.amount));
    put(&mut o, "status", s(&p.status));
    put(&mut o, "card_last4", s(&p.card_last4));
    Value::Object(o)
}

/// The table view of a payment: money as text, so the column reads `$123.45`.
pub fn payment_row(v: &Value) -> Value {
    let mut row = v.clone();
    if let Some(m) = v.get("amount") {
        row["amount"] = json!(money_text(m));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_dollar_values_become_money() {
        assert_eq!(money("$500,000").unwrap()["amount"], "500000.00");
        assert_eq!(money(" $1,234.56 ").unwrap()["amount"], "1234.56");
        assert!(money("Applied").is_none());
        assert!(money("(2% of Coverage A)").is_none());
    }

    #[test]
    fn labels_become_snake_case() {
        assert_eq!(snake("Primary applicant"), "primary_applicant");
        assert_eq!(snake("Co-applicant"), "co_applicant");
        assert_eq!(snake("Mortgagee clause"), "mortgagee_clause");
    }
}
