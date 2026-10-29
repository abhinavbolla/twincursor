## What and why

<!-- What this changes, and the problem it solves. Link the issue or roadmap item. -->

## Latency effect

<!-- Required if this touches capture, encoding, transport or rendering: before/after numbers
     from bench/run-matrix.sh or the network lab. Otherwise write "Not on the media path". -->

## New dependencies

<!-- One line per new crate: why it's needed, and its license (must be AGPL-compatible).
     Or "None". -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --release --workspace --all-targets -- -D warnings` and
      `cargo test --release --workspace` pass
- [ ] Tests added or updated for new logic
- [ ] No code copied from other pair-programming apps (clean room)
- [ ] Remote control stays consent-based (if this touches control)
- [ ] Docs updated if behaviour or commands changed
