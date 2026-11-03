# Security

## Secrets and personal data

- **Credentials live only in the OS keychain** (service `piekstra.kin`, one
  JSON item per login email holding the password, the session cookie jar and
  any login parked for its code), or in 1Password, read through `op` at the
  moment of use. Secrets enter via prompt, `--stdin`, `--from-env`, `--op`,
  or `KIN_PASSWORD` — never a command-line argument, file, or log line.
  A one-time code may be passed as `--code` (it is spent on first use and
  useless without the parked session); `--code -` reads it from stdin.
- **Logs and errors carry URL paths only.** Document downloads redirect to
  pre-signed S3 URLs whose query string is a working credential; `kin`
  never prints it and never sends the Kin session cookie to S3.
- **Downloaded documents are written owner-only** (mode 0600): declarations
  pages carry names, the property address and the mortgage loan number.
- **The repository carries no personal data.** Fixtures are scrubbed
  captures (`tests/fixtures/README.md`); `tests/no_personal_data.rs` scans
  every tracked file, and CI runs gitleaks. Never commit a raw capture.

## Account safety

Every login spends a one-time code, and repeated failed logins may lock the
account. `kin` never retries a rejected password, signs in again at most once
per command, and keeps the test suite offline.

## The MCP server

`kin mcp` serves read-only tools to an MCP client (Claude Code, Claude Desktop) over
stdio; it opens no port unless run with `--http` (below). **What the tools return goes to
Claude on every path**, as part of your conversation: policy numbers and status, the
property address, the term shown, coverage limits, deductibles, premiums, your lender's
name, document names, and upcoming and past payments. Applicants' names, the mortgage
loan number and the lender's other fields, a coverage's description, the policy's
features and discounts, a document's file name and description, and the last four digits
of the card on a payment are added when the model asks for `detailed`.

Document links work without a session, so no tool returns one, in any format. No tool
reads someone else's records: a policy reaches a path only after the signed-in login's
own list named it, and a `policy` argument is matched against that list, so a policy
number you did not get from Kin goes nowhere.

It reads the login from `${XDG_CONFIG_HOME:-~/.config}/kin/config.json` and the session
from the one keychain item on each call. That item also holds your password, so the
password is in memory while the item is read; it is dropped unused. A tool never signs
in, prompts, sends your password, reads your mailbox, asks for a one-time code, or creates
or changes a keychain item, so a background server cannot spend your codes or lock your
account, however often it restarts. Kin rotates the session cookie on each response; the
server keeps the new copy in memory until the stored session changes, and never writes it
to the keychain, since a write from a background server could block on a dialog. Only `kin`
commands advance the stored session, and every new server starts from it, which on stdio
means each launch or session of the client. So if it has gone stale, `kin auth login` is
needed even though the session was in use. A missing or lapsed session is an error asking
the user to run `kin auth login` in a terminal. A keychain error reaches the model only as
"could not be read": its detail can name the login email and quote the item, so `kin auth
status` shows that in the terminal. The Claude Desktop extension (`.mcpb`)
takes no settings and so holds no credential of its own; on macOS its bundled binary needs
the user's approval, once, to read the `piekstra.kin` keychain item.

The session goes to one host, `app.kin.com`, which is compiled in: no setting, variable or
file can redirect it.

Every tool's read goes through one seam that offers page reads alone. It has no way to send
a form, which is what every write at Kin takes, and the payments page's embedded payment
form is never read or posted. `tests/mcp_readonly.rs` drives every tool through the real
read path against a stub portal and fails on anything but a GET to a listed page.

`kin mcp --http --port <PORT>` serves the same tools over HTTP for the user's own
Cloudflare Worker, which reaches it through the user's own Cloudflare Tunnel (cli-common's
`connector/`). It binds 127.0.0.1 only, with no flag for another address, and refuses to
start without a bearer token of 32 or more characters, from `KIN_MCP_TOKEN` or the keychain
item `mcp-http-token` (`piekstra.kin`), which `kin mcp --new-token` makes from 256 random
bits and replaces only with `--overwrite`. Every request must carry the token (compared in
constant time) and a `Host` of `127.0.0.1` or `localhost`, and must not carry an `Origin`;
there are no CORS headers. Only the read-only tools are served (all four are). The Kin
session stays on your computer: the Worker sends tool calls and receives results, and never
sees it.

**Tool results also cross Cloudflare on this path**, through your tunnel and your Worker, on
their way to Claude. A tool result gives a property address, coverage and amounts, and a
detailed one gives names and a loan number, so treat the connector as you would anything
that reads those. Cloudflare is your own account's, and we run no shared service.

`kin auth logout --forget` removes the password, session, settings and `mcp --http` token.
It cannot reach a running `kin mcp --http`, which read its token at the start and keeps
accepting it until it stops, or a token in `$KIN_MCP_TOKEN`: stop the server (`launchctl
bootout` for its launchd agent), and unset the variable if it is set.

## Reporting

This is a personal, unofficial tool with no affiliation to Kin Insurance. If
you find a handling flaw (a secret reaching a log, a capture committed by
mistake), open an issue **without** including the offending value, or
contact the maintainer directly.
