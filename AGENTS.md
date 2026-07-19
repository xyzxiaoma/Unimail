<!-- TRELLIS:START -->

# Trellis Instructions

These instructions are for AI assistants working in this project.

This project is managed by Trellis. The working knowledge you need lives under `.trellis/`:

- `.trellis/workflow.md` — development phases, when to create tasks, skill routing
- `.trellis/spec/` — package- and layer-scoped coding guidelines (read before writing code in a given layer)
- `.trellis/workspace/` — per-developer journals and session traces
- `.trellis/tasks/` — active and archived tasks (PRDs, research, jsonl context)

If a Trellis command is available on your platform (e.g. `/trellis:finish-work`, `/trellis:continue`), prefer it over manual steps. Not every platform exposes every command.

If you're using Codex or another agent-capable tool, additional project-scoped helpers may live in:

- `.agents/skills/` — reusable Trellis skills
- `.codex/agents/` — optional custom subagents

Managed by Trellis. Edits outside this block are preserved; edits inside may be overwritten by a future `trellis update`.

<!-- TRELLIS:END -->

## Unimail repository rules

- Any user-visible change must update `CHANGELOG.zh-CN.md` under `未发布` in the same change. This includes UI copy or behavior, desktop capabilities, provider behavior, packaging, and installation behavior.
- Keep release notes in Simplified Chinese and describe the user impact rather than implementation details. Remove `暂无。` from a subsection when adding its first real entry.
- Never commit provider credentials, OAuth client secrets, local mail data, databases, signing certificates, updater private keys, notarization credentials, or `.env` files.
- Ordinary pushes may upload unsigned test installers as workflow artifacts, but must never create a GitHub Release. Release publication is tag-only and remains explicitly gated until the release integration work is complete.
