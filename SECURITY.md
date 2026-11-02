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

## Reporting

This is a personal, unofficial tool with no affiliation to Kin Insurance. If
you find a handling flaw (a secret reaching a log, a capture committed by
mistake), open an issue **without** including the offending value, or
contact the maintainer directly.
