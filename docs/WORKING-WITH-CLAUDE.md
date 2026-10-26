# Working with Claude on TwinCursor

A playbook for building this project efficiently with Claude Code.

## The loop (for every milestone item)

1. **New session, one task.** Start a fresh session per feature, e.g. "M1: capture one display
   and publish to LiveKit". Short, focused sessions beat one long session.
2. **Plan first.** Ask Claude to plan in plan mode, or run `/plan-eng-review` on bigger pieces.
   Read the plan, push back, then approve.
3. **Build on a branch.** Claude implements, writes tests, and runs build, test, and clippy.
   Use worktrees so two sessions can work in parallel on separate features.
4. **Prove it.** For media changes, ask for the latency numbers before and after.
5. **Review.** Run `/code-review` (or `/review`) before merging.
6. **Ship.** Run `/ship` to create the PR, then merge.
7. **Update the docs.** If something was learned, it goes in `docs/`. Tick the box in ROADMAP.md.

## Prompts that work well

- "Read docs/ROADMAP.md and start the next unchecked item in M1. Plan first."
- "Spike: try X in a throwaway branch and tell me if it works. Don't polish."
- "Write an ADR comparing ScreenCaptureKit vs libwebrtc DesktopCapturer for us."
- "This feels laggy: measure, find the slowest stage, propose one change, measure again."

## Useful skills and commands available here

| When | Use |
|---|---|
| Sharpen the idea and positioning | `/office-hours` |
| Lock an architecture or plan | `/plan-eng-review` |
| Debug something weird | `/investigate` |
| Review a diff | `/code-review`, `/review` |
| Open a PR | `/ship` |
| Security pass before a release | `/security-review`, `/cso` |
| Keep docs in sync after a merge | `/document-release` |

## Keep the context good

- `CLAUDE.md` is loaded every session. Keep it short and correct, and update the Commands section as code lands.
- Put long-lived knowledge in `docs/`, not in chat.
- If a session goes off track, start a new one rather than arguing with a long history.
