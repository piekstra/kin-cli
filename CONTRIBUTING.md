# Contributing

1. Read [AGENTS.md](AGENTS.md) and [docs/api.md](docs/api.md).
2. `make verify` must pass: `cargo fmt --check`, clippy with `-D warnings`,
   the offline test suite, and the smoke test.
3. A parser change comes with a fixture change: re-capture the page, scrub it
   per `tests/fixtures/README.md`, and update `tests/fixture_shapes.rs`.
4. Exercise the change through the installed binary (`make install`) before
   calling it done.
5. No personal data anywhere in the diff — source, docs, fixtures, commit
   messages.

`make install` and every other target that builds a binary sign it with the
`pk-cli-codesign` identity (`cli-common/scripts/setup-dev-signing.sh` creates
it once), so macOS keychain grants survive rebuilds.
