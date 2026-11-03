# kin-cli

Read-only command-line access to the [Kin Insurance](https://www.kin.com)
customer portal: your policies, their coverage limits and deductibles
(including Kin's flood endorsement), policy documents, and payments. It can
also serve them to Claude, read-only, over MCP.

Unofficial; not affiliated with Kin Insurance. Conforms to piekstra-cli spec v1.

```console
$ kin policies list
NUMBER | STATUS | KIND | ADDRESS | FLOOD_ENDORSEMENT
KIN-DP-FL-000000001 | Active | Homeowner | 100 Example St, Exampleton, FL 00000-0000 | true

$ kin policies get --term previous
$ kin documents download declaration_page -o declarations.pdf
$ kin payments list --all
```

## Install

```sh
cargo install --git https://github.com/piekstra/kin-cli --locked
# or, from a checkout (also signs the binary on macOS):
make install
```

## Sign in

Kin signs in with your password and a one-time code sent by email or text.

```sh
kin auth login --username you@example.com          # prompts for the password, then the code
```

To make later sign-ins unattended:

```sh
kin config set op_ref op://Private/Kin/password     # password from 1Password
kin config set mailbox gro:work                     # read the emailed code with gro
```

`auth login` stores the password it used and the session in the OS keychain
(service `piekstra.kin`, one item). When Kin lets the session lapse, the next
command signs in again by itself if it can read the code from the mailbox;
otherwise it asks for the code on a terminal, or exits 3 with the login
parked:

```sh
kin auth login --code 123456                        # finish a parked login
```

## Commands

| Command | Output |
|---|---|
| `auth login` / `status [--verify]` / `logout [--forget]` / `set-credential` | `auth-status/v1` |
| `policies list` | `policy-list/v1` |
| `policies get [POLICY] [--term current\|previous\|YYYY-MM-DD]` | `policy/v1`: coverage limits, deductibles, flood endorsement and its premium, other features, discounts, lender |
| `documents list [--policy P] [--term T]` | `document-list/v1` (documents/v1 profile) |
| `documents download <ID> [-o PATH\|-]` / `--all -o DIR` | `document-download/v1` / `document-download-batch/v1` |
| `payments list [--past\|--all] [--policy P]` | `payment-list/v1` |
| `config path\|show\|set\|unset` | settings: `username`, `policy`, `otp_channel`, `mailbox`, `op_ref`, `secret_sources` |
| `mcp [--list-tools] [--new-token \| --http --port N]` | serves the read-only tools to Claude ([below](#use-with-claude)) |
| `info`, `completions <shell>`, `self-update` | the standard family commands |

Every command takes `--json` (one schema-tagged document on stdout), `-v`,
`-q`. Exit codes: 0 ok, 2 usage, 3 sign-in needed, 4 not found, 5 Kin
answered unexpectedly.

`POLICY` is a policy number or a unique part of its address; the
default is the configured `policy`, else the account's only (or only active)
policy.

## Use with Claude

`kin mcp` serves four read-only tools to Claude over MCP: `kin_policies`, `kin_policy`,
`kin_documents` and `kin_payments`. Nothing can pay a bill, change a policy or its
coverage, file a claim or change anything at Kin. Document results name documents and
carry no link and no contents; you save one with `kin documents download`.

**Claude Code.** After `kin auth login`:

```sh
claude mcp add kin -- kin mcp
```

Or install the `kin-cli` plugin from
[claude-plugins](https://github.com/piekstra/claude-plugins), which runs the same
command.

**Claude Desktop.** Download `kin-<version>.mcpb` from the
[latest release](https://github.com/piekstra/kin-cli/releases/latest) and open it. It
asks for nothing: the bundled `kin` reads the session your installed `kin` stored. On
macOS the bundled binary is a different program to the keychain, so its first call
raises a keychain prompt for the `piekstra.kin` item; choose **Always Allow**, or every
call will ask again.

Any other MCP client that launches a local server can run `kin mcp`.

Unlike the commands above, the tools never sign in, never send your password and never
ask for a one-time code, since each sign-in spends one. When Kin lets the session lapse
they say so, and you run `kin auth login` again. Plan on that: the server keeps Kin's
rotated session cookie in memory only, Claude starts a new server for each launch or
session, and each new one starts from the session last stored, which only `kin` commands
advance. How long Kin lets an idle session live has not been measured. `kin auth status
--verify` advances the stored session without signing in, as long as it still works.

**Claude on the web and mobile.** claude.ai reaches tools through a remote connector,
and your Kin session stays on your computer, so `kin` serves the same tools from your
computer to a connector you deploy into your own Cloudflare account:

```console
$ kin mcp --new-token            # once: a bearer token, saved in the keychain and printed for the Worker
$ kin mcp --http --port 8753     # serve on 127.0.0.1 only
```

`--http` takes the token from `$KIN_MCP_TOKEN` if it is set, then from the keychain.

The connector, [cli-common's `connector/`](https://github.com/piekstra/cli-common/tree/main/connector#tools-from-your-computer-home-modules),
reaches that port through a Cloudflare Tunnel in your account; its README walks through
the tunnel, the VPC service, a launchd agent that keeps the server running, and the
`HOME_KIN_TOKEN` secret, with the threat model. `--http` binds 127.0.0.1 only, refuses to
start without the token, refuses any request whose `Host` is not loopback or that carries
an `Origin`, and serves the read-only tools only. Your Kin password and session never
leave your computer, but tool results do: policy numbers, the property address, coverage,
premiums, your lender's name, document names and payments pass through Cloudflare and
reach Claude, and detailed results add applicants' names and the loan number
([`SECURITY.md`](SECURITY.md)). While the computer sleeps, the tools answer that it is
offline.

After `kin self-update`, restart the server: a running one keeps the old code, and its
launchd agent restarts it only when it exits. Its start line names the version.

```console
$ launchctl kickstart -k gui/$(id -u)/com.piekstra.kin-mcp-http
```

`self-update` does not reach a Claude Desktop extension, which runs its own copy.
Install the new release's `.mcpb` too.

## How it works

The portal has no API; `kin` reads its server-rendered pages over plain
HTTPS. [docs/api.md](docs/api.md) records every request, the hooks the
parsers rely on, and the traps.

## License

MIT or Apache-2.0.
