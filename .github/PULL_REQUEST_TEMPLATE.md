## What and why

<!-- What does this change, and why? Link the issue. -->

## Checklist

- [ ] Tests added/updated (and they fail without the change)
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] Docs updated (`docs/`, and `python3 scripts/gen_cli_docs.py` if a command or flag changed)
- [ ] `CHANGELOG.md` updated for user-visible changes
- [ ] No change to hashing, on-disk formats, merge semantics or the publication protocol — or an ADR in `docs/adr/` explains it
