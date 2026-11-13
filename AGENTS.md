# Agent instructions

## Find the right context

- Start with [`docs/current-state.md`](docs/current-state.md) for the current milestone and
  code map. Read only the architecture section, ADRs, plan, and tests relevant to the task.
- [`docs/roadmap.md`](docs/roadmap.md) is a long development history. Its opening links to
  the maintained status snapshot; do not read the whole history as routine task setup.
- Check for a closer `AGENTS.md` in the files' directory tree before editing. Keep this
  file as shared guidance for all coding agents; put tool-specific details in that tool's
  own instruction file.

## Preserve project invariants

- Keep `crates/clustine-sim` deterministic: no I/O, clock reads, or unordered maps.
- Keep Minecraft packet handling in `services/edge`; workers consume semantic inputs
  ([ADR-0005](docs/adr/0005-edge-worker-interface.md)).
- Tests wait for a message or state, not a fixed delay. Use `expect` only for documented
  invariants; do not add `unwrap` to production code.
- Follow nearby conventions: British spelling, comments that explain why, and sentence
  names in `snake_case` for tests.
- Update the current-state snapshot and affected parity/architecture docs when the task
  changes what exists, what is next, or a project-wide contract.

## Verify proportionally

Use [`docs/agent-workflow.md`](docs/agent-workflow.md) to pick the smallest useful check
first, then expand to workspace, oracle, or cluster checks when the change warrants it.
Report exactly which checks ran and their result. Do not imply that an unrun check passed.

Do not commit or push unless the task explicitly asks for it. When a task authorizes
commits, the owner's preference is one verified commit per step on `main`, without a PR.
