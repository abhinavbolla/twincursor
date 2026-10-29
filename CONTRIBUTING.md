# Contributing to TwinCursor

Thanks for helping! TwinCursor is a remote pair-programming app: low-latency screen sharing,
voice and remote control, all in Rust.

## How to contribute

1. **Pick something.** Start with [docs/ROADMAP.md](docs/ROADMAP.md) and the current
   milestone, or an open issue. For anything bigger than a small fix, open an issue first so we
   can agree on the approach.
2. **Fork** the repository and create a branch in your fork.
3. **Build and check** (macOS needs Xcode 26):
   ```bash
   cargo fmt --all
   cargo clippy --release --workspace --all-targets -- -D warnings
   cargo test --release --workspace
   ```
   [CLAUDE.md](CLAUDE.md) has the full list of commands, including the latency benchmarks and
   the network lab.
4. **Open a pull request** against `main` and fill in the template. CI runs on it once a
   maintainer approves the run (required for first-time contributors).
5. **Sign the CLA** when the bot asks, by posting the comment it suggests on your pull request.
   You only do this once. See [CLA.md](CLA.md).

`main` is protected: every change goes through a pull request with passing CI, and a
maintainer merges it.

## Ground rules

1. **Clean room.** Do not copy code from other pair-programming apps (for example Hopp or
   Tuple), even open-source ones. Ideas from public blog posts and talks are fine; write your
   own implementation.
2. **Measure media changes.** PRs that touch capture, encoding, transport or rendering must
   include before and after latency numbers (see
   [Architecture](docs/ARCHITECTURE.md#measuring-latency)). No unmeasured optimizations.
3. **Remote control stays consent-based.** The sharer must grant control explicitly, see who
   is controlling, and be able to revoke it instantly. Changes that weaken this won't be
   merged.
4. **New dependencies** need a one-line reason in the PR, and an AGPL-compatible license (MIT,
   Apache-2.0, BSD, MPL-2.0, LGPL, GPL-3.0 or AGPL-3.0).
5. **Small PRs**, reviewable in about 15 minutes, with
   [Conventional Commit](https://www.conventionalcommits.org) messages (`feat:`, `fix:`,
   `perf:`, `docs:`, `chore:`).

## License and CLA

TwinCursor is licensed under [AGPL-3.0](LICENSE). Contributions are accepted under the
[Contributor License Agreement](CLA.md). You keep the copyright to your work. The CLA gives the
project the right to use it, and to offer TwinCursor under other licenses too, for example a
commercial license, without asking every contributor again.

## Security

Please don't report security problems in public issues. See [SECURITY.md](SECURITY.md).
