# Sub-Agent Orchestration

**This is a required instruction file for agents working in this repo, linked from
the project-root `CLAUDE.md`. Read it before dispatching any sub-agent work.**

This project uses parallel sub-agents by default to avoid unnecessary waiting.
Follow these rules for every new request:

## Default posture: dispatch, don't block
Substantial or self-contained work (a feature, a widget change, a refactor,
research) should be started as a **background sub-agent** rather than worked on
inline in the main thread — even when nothing else is running. The main thread
stays free to accept the user's next message. Only trivial, one-line, or
tightly-coupled edits are done inline. When multiple such tasks arrive, fan them
out to one background agent each and merge their worktree branches as they finish.
Agents that edit files run with `isolation: worktree`; if several touch the same
file but in far-apart regions, their branches still merge cleanly.

## Batching a large request (many edits at once)
When one request contains many independent edits, fan them out — but partition by
*which files they touch*, not just by feature, so integration stays conflict-free:
- **Worktree agents branch from HEAD.** This repo sets `worktree.baseRef: head` in
  `.claude/settings.json` (local; `.claude/` is gitignored). Never let agents branch
  from `master` when the working branch is ahead, or every result needs hand-merging.
- **Only ONE agent per batch may edit the shared scaffolding files** — `config.rs`
  (enums, `AppConfig` fields, `MINISETTINGS_KEYS`), `app.rs` (mini-settings sub-tabs),
  `i18n.rs`. Two agents each adding a config field + mini-setting + translation in
  parallel WILL collide on those regions. Hand all scaffolding-touching tasks to one
  agent, or sequence them; let the others be pure-rendering edits.
- **Pure-rendering edits to the same file are parallel-safe if they touch different
  functions.** `dashboard.rs` is huge, so several agents each editing a different
  widget function cherry-pick cleanly.
- **Integrate sequentially, per agent as it lands** (not one big merge at the end):
  `git cherry-pick <agent-commit>` onto HEAD → `cargo build` (+ `cargo test` if config
  changed) → remove the worktree and delete its branch. Incremental verification pins a
  bad agent immediately; a dedicated end-of-run "merge agent" is not worth it —
  conflicts are cheaper to prevent by partitioning than to resolve after.
- **Removing/renaming a config enum value or field** needs a serde migration in
  `AppConfig::load` (see the "Light"→"Dark" theme fix-up) plus updates to bundled
  presets, or old `config.json` files fail to parse and reset.

## Core Rule
If a new user request arrives while a task is already running
(or if a request contains multiple independent subtasks):
- Immediately start a separate sub-agent via the Task tool for each
  independent subtask, instead of working sequentially.
- Do NOT wait on your own initiative until the current task is finished
  before accepting the new one — first check whether a dependency exists.

## When to Dispatch in Parallel (all conditions must be met)
- The new task does not need the result of the currently running task.
- There is no shared state (e.g. the same file being edited).
- The task is clearly scoped and self-contained.

→ In this case: spawn a new sub-agent in parallel (Task tool), keep the
main thread free for further input.

## When to Work Sequentially / Wait (any one condition applies)
- The new task depends on the result of the currently running task.
- Both tasks touch the same files or the same code section
  (merge conflict risk).
- The scope of the new task is unclear — clarify or briefly analyze
  before delegating.

→ In this case: give a short heads-up ("waiting on X before starting Y")
and only start / attach afterward.

## Background Dispatch
- Research, analysis, and read-only tasks (no file changes) should
  generally be started as background agents so the main thread doesn't block.
- File-modifying tasks (edit/write) should only be run in parallel if they
  touch different, clearly separated directories/files.

## Isolation for Parallel File Changes
- Sub-agents that modify code while running in parallel with others use
  `isolation: worktree` so agents don't overwrite each other.
- Each agent commits its changes to its own worktree branch.

## Reporting
- Each sub-agent returns a short, concise summary at the end
  (no raw logs, no intermediate steps).
- When multiple sub-agents run in parallel, consolidate their results
  into a single overview in the main thread at the end.
- Briefly state what each spawned sub-agent is responsible for
  (e.g. "Sub-agent A: Auth research" / "Sub-agent B: DB layer refactor").

## Model and Effort Selection for Sub-Agents (dynamic)

The main agent decides independently which model and which effort level
are appropriate for a task. The following values are guidelines, not fixed
rules — deviate from them whenever the task calls for it.

**Available models:** Sonnet, Opus, Ultracode
**Available effort levels:** Low, Medium, High, XHigh

### Guidelines for Estimation
- **Small, simple adjustments** (e.g. value changes, simple formatting,
  trivial config edits) → Sonnet, Medium
- **Tasks involving logic** (e.g. new features, business logic,
  non-trivial refactoring) → Opus, High/XHigh
- **Bug fixing** → ALWAYS Opus, at least High, XHigh for complex/unclear bugs
- **Very complex or critical tasks** (e.g. architecture decisions,
  security-relevant code, hard-to-trace bugs) → Ultracode, XHigh if needed

This mapping is guidance only. The main agent should assess the actual
complexity of each subtask itself and choose model/effort accordingly —
even if that means deviating from the guidelines (e.g. Opus for a
seemingly simple but sensitive task).

### User Override
- If I (the user) explicitly name a model or effort level
  (e.g. "use Sonnet for this" or "with low effort"), that ALWAYS takes
  precedence over the automatic estimation — switch immediately.
- Without explicit input from me, the main agent decides independently
  based on the guidelines above.

## Limits
- Sub-agents cannot spawn further sub-agents themselves (no nested
  delegation). For multi-step workflows, let the main thread coordinate
  instead of nesting deeply.
- Maximum of 4 parallel sub-agents at once, to keep token cost and
  rate limits in check.

## How this maps to the runtime (accuracy notes)
- **Background is real.** Sub-agents launched with the Task/Agent tool run
  in the background by default; the main thread is notified when each one
  finishes and keeps working in the meantime. The main agent decides per
  subtask *when* to start it and *whether* to block on another task's result
  — that dependency check is the whole point of the rules above.
- **The main agent is event-driven, not always-on.** It acts when the user
  sends a message or when a background sub-agent completes — it does not poll
  on its own between those events. In practice this still covers the core
  rule: a new request that arrives mid-work is seen and can be dispatched to a
  parallel agent immediately, without waiting for the running task (unless a
  dependency or shared file forces sequencing).
- **"Ultracode" is a mode, not a model.** It refers to multi-agent workflow
  orchestration. The actual selectable sub-agent models are Sonnet / Opus /
  Haiku; read "Ultracode, XHigh" as "use a workflow and/or the highest effort."
