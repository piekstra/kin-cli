# Fixtures

Scrubbed captures of `app.kin.com` pages, read by `tests/fixture_shapes.rs`.

Captured 2026-11-02 from a signed-in session with a plain HTTP client (the
same requests `kin` makes).

| File | Page |
|---|---|
| `portal.html` | `/kin_portal` — the policy cards |
| `policy_details.html` | `/kin_portal/policies/<number>/details` |
| `documents.html` | `/kin_portal/policies/<number>/documents` |
| `payments_upcoming.html` | `/kin_portal/policies/<number>/payments?billing_status=upcoming` |
| `payments_past.html` | `/kin_portal/policies/<number>/payments?billing_status=approved` |
| `sign_in.html` | `/users/sign_in` — the login form only |

## Scrubbing policy

Structure is preserved exactly inside the parts kept: tags, attributes,
class names, `data-rspec`/`data-testid` hooks, nesting and whitespace. What
was removed or replaced:

- **Trimmed:** everything outside the `policy-content` turbo frame (the
  portal page: outside `<main>`), so the page shell, analytics and consent
  scripts are gone. A dummy sign-out form is prepended, because a signed-in
  page is recognized by that form and it lives in the trimmed header.
- **Inline SVG** icons are emptied (`<svg></svg>`).
- **Identity:** names → `Pat Example` / `Sam Example`; the property address
  → `100 Example St, Exampleton, FL 00000-0000`; the policy number →
  `KIN-DP-FL-000000001`; lender name, loan number, address, fax and email →
  `Example Mortgage Co`, all zeros, `555` numbers, `lender@example.com`;
  card digits → `0000`; masked contacts → `***-***-0000`,
  `p......@example.com`; Kin's phone number → a `555` number.
- **Money:** every digit of every `$` amount → `1` (`$500,000` →
  `$111,111`). Shape and formatting are kept; the values mean nothing.
- **Dates:** term dates → January 1 of their year; payment dates → the 1st
  of their month, 2026.
- **Credentials:** CSRF tokens → `dummy-csrf-token`; `active_storage`
  signed ids → `DUMMY--000…`; the payment processor's client key →
  `test_DUMMYCLIENTKEY`; transaction ids → `1`.

`tests/no_personal_data.rs` enforces this over every tracked file. Never
commit a raw capture: the session cookie and the `active_storage` links in
it are live credentials.
