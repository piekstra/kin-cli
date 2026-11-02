# CLAUDE.md

See **[AGENTS.md](AGENTS.md)** — the canonical guide for this repo (layout,
the auth model, conventions, tests, definition of done). This file only adds
Claude-specific notes.

- A live login spends a one-time code and may count toward a lockout. Use the
  fixtures; log in only when the owner asks for a live check.
- If a `kin` call hangs, it is waiting on a macOS Keychain or 1Password
  approval. Stop calling it and tell the owner; a retry only raises another
  dialog.
- Read `docs/api.md` before changing a parser or the login flow, and update it
  in the same change.
