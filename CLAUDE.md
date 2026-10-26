# TwinCursor

Open-source (AGPL-3.0) remote pair-programming app, an alternative to Tuple: low-latency
screen sharing, voice, and full remote control of the other person's machine.

Read before larger work:
- `docs/PRODUCT.md`: what we're building and for whom
- `docs/ARCHITECTURE.md`: how the pieces fit together, and why
- `docs/ROADMAP.md`: milestones; work on the current one only
- `docs/research/hopp-lessons.md`: what we learned from Hopp's public engineering blog

## Hard rules

1. **Clean room: never copy code from Hopp (github.com/gethopp/hopp) or Tuple.** Do not open,
   fetch, paste, or paraphrase their source files. Learning from their *public blog posts* and
   docs is fine; implement ideas in our own code. If a task seems to need their code, stop and ask.
2. **Media never runs in the webview.** Capture, encode, WebRTC, decode and rendering of the
   shared screen live in Rust (`crates/`). The Tauri webview is only for app UI (lobby, settings,
   call controls). This is the main lesson from Hopp; see the research note.
3. **Latency is the product.** Any change to the media path must report its effect on
   glass-to-glass latency (see "Measuring latency" in ARCHITECTURE.md). No unmeasured "optimizations".
4. **Remote control is consent-based.** The sharer must explicitly grant control, see who is
   controlling, and be able to revoke it instantly with a global hotkey. Never weaken this.
5. New dependencies need a one-line justification in the PR description. Licenses must be
   AGPL-compatible (MIT, Apache-2.0, BSD, MPL-2.0, LGPL, GPL-3.0, AGPL-3.0).

## Repo layout (planned; create as milestones need them)

```
apps/desktop/     Tauri 2 shell (Rust + TypeScript/React UI)
crates/media/     screen capture, encoding, LiveKit/WebRTC session, native viewer
crates/control/   remote input: event protocol, injection (enigo), cursor overlay
crates/protocol/  shared message types (serde), versioned
server/           Rust (axum) API: auth, rooms, LiveKit token issuing
deploy/           docker compose for self-hosting (server + LiveKit + Postgres)
docs/             product, architecture, roadmap, research, ADRs
```

## Commands

Fill these in as code lands; keep them accurate.

- Build all Rust: `cargo build --workspace`
- Test all Rust: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format: `cargo fmt --all`
- Desktop dev: _TBD (M4)_
- Local LiveKit: _TBD (M1)_

## Conventions

- Rust edition 2021+, `cargo fmt` and clippy-clean. Errors: `thiserror` in libraries, `anyhow` in binaries.
- Prefer small, focused crates with clear boundaries over one large crate.
- TypeScript: strict mode, functional React components, no `any`.
- Commits: Conventional Commits (`feat:`, `fix:`, `perf:`, `docs:`, `chore:`).
- Architecture decisions go in `docs/adr/NNNN-title.md` (short: context, decision, consequences).
- Platform-specific code goes behind `cfg(target_os = ...)` modules with a shared trait.

## Working agreement for Claude

- Before a milestone or any multi-file change: plan first (plan mode), and confirm the plan.
- One feature per session/branch. Keep PRs small enough to review in ~15 minutes.
- Write or update tests alongside code; run build, test, clippy before saying something works.
- When you learn something non-obvious (a platform quirk, a latency finding), add it to the
  relevant doc rather than leaving it only in chat.
- macOS is the first platform. Keep code portable, but don't build Windows or Linux paths before their milestone.
