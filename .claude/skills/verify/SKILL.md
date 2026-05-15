---
name: verify
description: Run scribed's verify chain — rustfmt check, strict clippy, nextest, doc tests — and report the first failure. Use before declaring any code change complete, or when the user says "verify", "check this", or "make sure CI will pass".
---

Run the four commands below in order. Stop at the first non-zero exit and report what failed, what the output said, and where to look. If all four pass, report each command's pass status in one line.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
cargo test --doc
```

Notes:

- `cargo nextest run` is the project's test runner — do not substitute `cargo test`.
- The `daemon_lifecycle` integration test downloads the ~442 MB Nemotron bundle to `~/.cache/scribed/` on first run. If the user wants to skip it for speed, run `cargo nextest run -E 'not test(daemon_lifecycle)'` and tell them which test was excluded.
- Clippy denies all warnings. If clippy fails, fix the underlying lint — do not add `#[allow(...)]` unless the user explicitly approves.
- Doc tests are fast; always run them.
