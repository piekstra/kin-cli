# The Kin customer portal, as `kin` uses it

`app.kin.com` is a Rails application that publishes no API. Every page is
server-rendered HTML (Turbo), and `kin` reads them with a plain HTTP client.
There is no bot check in front of the login, so no browser is involved.

## Requests

| Step | Request | Success looks like |
|---|---|---|
| Login form | `GET /users/sign_in` | 200; the `<form action="/users/sign_in">` carries an `authenticity_token` |
| Password | `POST /users/sign_in` with `authenticity_token`, `user[email]`, `user[password]`, `user[remember_me]=0`, `commit=Log in` | 302 to `/users/one_time_password` |
| Code channels | `GET /users/one_time_password` | radio inputs `otp_method` = `text` / `email`, labels "Text a code to …" / "Email a code to …" with a masked destination |
| Send code | `POST /users/one_time_password` with `authenticity_token`, `otp_method`, `commit=Send code` | 302 to `/users/verify_one_time_password?otp_contact=<masked>&otp_method=<channel>` |
| Verify form | `GET` that URL | the `<form action="/users/verify_one_time_password">` token |
| Code | `POST /users/verify_one_time_password` with `authenticity_token`, `otp_attempt`, `commit=Submit` | 302 to `/kin_portal?source=login` |
| Policies | `GET /kin_portal` | one `div.policy-summary` per policy |
| Details | `GET /kin_portal/policies/<number>/details[?term=<value>]` | coverage cards (below) |
| Documents | `GET /kin_portal/policies/<number>/documents[?term=<value>]` | `div.policy-document__item` rows |
| Payments | `GET /kin_portal/policies/<number>/payments?billing_status=upcoming\|approved` | `div.billing-transaction__item` rows |
| Sign out | `POST /users/sign_out` with `_method=delete`, `authenticity_token` | 302 |

A signed-in page is recognized by its sign-out form
(`action="/users/sign_out"`); the sign-in page by `name="user[password]"`.

## The code email

From `notification@help.kin.com`, subject "Requested One-Time Password".
Its snippet reads "Your requested one-time password is: NNNNNN", so the
shared extractor finds the code without opening the message. Kin also texts
the same code to the phone on file when the email channel is chosen.

## Hooks the parsers key on

Kin's own tests tag the coverage page with `data-rspec`, which makes those
attributes the most stable hooks available:

| Hook | Meaning |
|---|---|
| `<key>-label`, `<key>-value-1` | one coverage or feature line; `coverage_a`…`coverage_m` are the primary limits, everything else (`flood_and_water_backup`, `sinkhole`, …) an "other feature" |
| `<key>-help-text-content-1` | the line's description |
| `standard-deductible`, `hurricane-deductible` | deductible amounts; `hurricane-deductible-percentage` holds "(2% of Coverage A)" |
| `flood-premium` | the flood endorsement's premium for the shown term — listed inside the deductibles card |
| `data-document-label` | a document's stable id (`declaration_page`) |
| `data-testid="billing-transaction-amount"` | an upcoming payment's amount |

Everything else is read from BEM class names (`policy-summary__policy-number`,
`policy-detail__item`, `discounts__item-name`, `billing-transaction__item`),
never from layout classes.

## Traps

- **A lapsed session is a redirect, not a 401.** A page request without a
  valid session answers 302 to `/users/sign_in`. A client that follows
  redirects turns that into a 200 login page, so `kin` follows them by hand
  and treats a sign-in redirect or a rendered login form as "signed out".
- **A session cookie does not mean a session.** The anonymous redirect to
  sign-in also sets a fresh `_bright-policy_session`. Whether a stored
  session existed has to be decided before the first request.
- **The session cookie is re-issued on every response.** `kin` writes the
  jar back to the keychain after each command.
- **Document links leave the portal.** `/rails/active_storage/blobs/redirect/…`
  answers 302 to a pre-signed URL on `kin-documents-production.s3.amazonaws.com`.
  The session cookie must go only to `app.kin.com`, and the S3 query string is
  a working credential: it never appears in a log or an error.
- **The header ignores `?term=`.** "Current premium" and "Policy period" on
  the details page describe the policy as it stands, whichever term is
  selected; only the cards below follow the term. The DTO names them
  `current_premium` and `current_period`.
- **Term selectors differ per page.** Details uses values `1` (current) and
  `0` (previous) with dated labels; documents uses `current_term` and `0`, and
  labels the current term "Current Term" without dates. Terms run back to
  back, so an undated current term starts where the previous one ends.
- **A failed password post answered 401** (observed with an empty password),
  not a re-rendered form. Repeated failures may lock the account, so a
  rejected password is never retried automatically.
- **The payments page embeds a live payment form** (Adyen, "Pay" submits
  immediately). `kin` is read-only and never posts to it.

## What `kin mcp` reads

The policies, details, documents and payments rows above, and nothing else: GETs alone,
with the stored session, through `mcp::LivePortal`. A number or `?term=` value in a path
comes from the portal's own page, never from a tool argument. It treats what the commands
tolerate as an error, so a model is not told a wrong answer: no policy card, a card or row
the parsers could not read, no document, a payments page without its
`turbo-frame#billing-transactions`, an amount in that frame with no row read, a row whose
amount is not money, and a page for another term still showing its `selected` option on
the first one.

## Not yet verified

- How long an idle session lasts before Kin redirects to sign-in.
- Whether checking "Remember me" (`user[remember_me]=1`) skips the code step
  on later logins.
- The markup of an account with more than one policy, or of a cancelled or
  expired policy (only one active policy has been observed).
- The claims pages (`/kin_portal/claims`, `claims.kin.com`), which `kin` does
  not read.

Update trigger: revise this page whenever a parser or the login flow changes,
or a fixture is re-captured.
