# kin-cli

Read-only command-line access to the [Kin Insurance](https://www.kin.com)
customer portal: your policies, their coverage limits and deductibles
(including Kin's flood endorsement), policy documents, and payments.

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
| `info`, `completions <shell>`, `self-update` | the standard family commands |

Every command takes `--json` (one schema-tagged document on stdout), `-v`,
`-q`. Exit codes: 0 ok, 2 usage, 3 sign-in needed, 4 not found, 5 Kin
answered unexpectedly.

`POLICY` is a policy number or a unique part of its number or address; the
default is the configured `policy`, else the account's only (or only active)
policy.

## How it works

The portal has no API; `kin` reads its server-rendered pages over plain
HTTPS. [docs/api.md](docs/api.md) records every request, the hooks the
parsers rely on, and the traps.

## License

MIT or Apache-2.0.
