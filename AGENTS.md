# AGENTS.md — kin-cli

`kin` is a read-only CLI for the Kin Insurance customer portal
(`app.kin.com`): policies, coverages and deductibles, documents, payments.
It conforms to the piekstra-cli/1 spec and is built on the `pk-cli-*` crates
from [cli-common](https://github.com/piekstra/cli-common).

## Layout

| File | Owns |
|---|---|
| `src/main.rs` | the clap tree, argument checks, dispatch, exit codes |
| `src/client.rs` | HTTP to `app.kin.com` with hand-followed redirects; the login (`KinLogin`, an `OtpTransport`) |
| `src/session.rs` | the shared one-time-code flow wired to Kin, and `Portal`: a read that signs in again once when the session lapsed |
| `src/parse.rs` | total parsers over the server-rendered pages |
| `src/dto.rs` | the JSON shapes and the `policies get` text view |
| `src/bundle.rs` | the one keychain item and its two views (password source, parking slot) |
| `src/jar.rs` | the cookie jar that survives between runs |
| `src/config.rs` | non-secret settings |
| `docs/api.md` | every request, hook and trap — read it before touching a parser |

## The auth model

Login is password plus a one-time code (email or text). The flow is
`pk_cli_auth::otp::OtpLogin`; `KinLogin` supplies the two provider steps.

- The password resolves through `SecretResolver`: `KIN_PASSWORD`, then the
  keychain bundle, then 1Password (`op_ref`), in `secret_sources` order.
  `auth login` stores the password it used in the bundle, so later
  re-logins need no 1Password approval.
- With `mailbox` set (`gro` or `gro:<profile>`), the emailed code is read
  through `gro` and the login finishes unattended. Without it, the code is
  prompted for on a TTY; otherwise the login is parked and the command exits
  3 naming `kin auth login --code <CODE>`.
- `GroMailbox::credential_ref` passes `--ref`, which gro 2.x removed, so a
  named profile is selected with `GOOGLE_READONLY_CREDENTIAL_REF` instead
  (`session::mailbox`).
- Every read replays the stored session. A redirect to sign-in triggers one
  re-login and one retry (`Portal::page`).

## One keychain item

Service `piekstra.kin`, account = the login email, one JSON item holding the
password, the session jar and any parked login (`Bundle`). One item because
every item a freshly built binary reads is its own macOS prompt. Every
`make` target that produces a binary signs it with the stable
`pk-cli-codesign` identity, so a rebuild keeps the "Always Allow" grant.

## Non-negotiable conventions

- `--json` on every command, one schema-tagged DTO on stdout, diagnostics on
  stderr. Exit codes: 0 ok, 2 usage, 3 auth, 4 not found, 5 upstream.
- Arguments are validated before the keychain or the network is touched.
- Secrets never go on argv, into a log, or into an error. Errors and
  `--verbose` lines carry URL paths only (`client::path_of`): a document
  link's query is a pre-signed credential.
- Read-only. The payments page embeds a live payment form; nothing in `kin`
  posts to it. The first write would need SPEC §1.3's confirmation gate and
  a change to this paragraph in the same commit.

## Success is never a status code

A page counts only when it renders signed in (`parse::is_signed_in`). A code
counts only when the portal then loads signed in. A download counts only
after `verify_download` sees a PDF. A parser that finds nothing is exit 5
("page changed?"), never an empty success.

## Tests are offline, always

`cargo test` never touches the keychain or the network:

- `tests/cli_surface.rs` runs the binary with `XDG_CONFIG_HOME` pointed at a
  per-test directory (unique by counter, not clock), so no test inherits a
  configured login.
- `tests/fixture_shapes.rs` runs the parsers over scrubbed captures in
  `tests/fixtures/` (scrubbing policy in its README).
- `tests/no_personal_data.rs` scans every tracked file for personal-data
  shapes.

Every live login spends a code and Kin may rate-limit or lock the account.
Never log in to "check something"; use the fixtures.

## Definition of done

`make verify` green, the change exercised through the **installed** binary
(`make install`, then every affected command with and without `--json`),
`docs/api.md` matching reality, and no personal data in the diff.
