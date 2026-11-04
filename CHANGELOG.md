# Changelog

All notable changes to `kin` are documented here. Newest first. The project follows
[SemVer](https://semver.org/); until 1.0, minors carry the breaking-change signal.

## v0.2.1 — 2026-11-04

- `auth logout --forget` says what the connector token's state leaves true, and claims a removal only when there was an item: a running `kin mcp --http` keeps the token it started with, and one on `$KIN_MCP_TOKEN` never had an item to remove.
- cli-common v0.29.0, up from v0.22.0, which brings the Windows config and self-update fixes.

## 0.2.0 — 2026-11-03

- `kin mcp`: four read-only tools for Claude: `kin_policies`, `kin_policy`, `kin_documents`
  and `kin_payments`. They run over stdio for Claude Code and Claude Desktop, and with
  `--http` on 127.0.0.1 behind a bearer token for Claude on the web and mobile through your
  own Cloudflare Worker and tunnel. No tool can pay a bill, change a policy or its coverage,
  file a claim or change anything at Kin.
- `kin mcp` never signs in, sends the password, asks for a one-time code, reads the mailbox
  or writes the keychain, and reads the config and the session on each call. A lapsed session
  is an error naming `kin auth login`. A changed page is an error rather than an empty list.
  Every other command is unchanged.
- `kin auth logout --forget` also removes the `mcp --http` token, and says what a running
  server and `$KIN_MCP_TOKEN` still need.
- Releases attach `kin-<version>.mcpb`, a Claude Desktop extension, and a Windows x86_64
  tarball.
- `self-update` says to restart a running `kin mcp --http` and to install the new `.mcpb`.
- cli-common v0.22.0, up from v0.15.1. Rust 1.89 or later.
