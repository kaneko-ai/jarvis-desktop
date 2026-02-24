# AGENTS.md
# Jarvis Desktop — Agent Operating Rules (Codex / VSCode / CLI)

## 0. Purpose (top priority)
This repo (jarvis-desktop) incrementally ports useful features from jarvis-ml-pipeline into a stable desktop UI.
The ultimate value is not just "it works" but that it does NOT break under long-running operation:
- Reproducibility (same operation, same result)
- Observability (fast root-cause via logs/diagnostics)
- Safety (no path traversal accidents)
- Diff hygiene (reviewable, rollback-friendly)

## 1. Agent Rules
### 1.1 Always
- Never guess specs: use rg / existing code / existing tests for evidence
- Keep diffs small: target 200-400 lines per PR. Split if larger.
- Preserve backward compat: keep existing Tauri commands and UI flows
- Respect safety zones: run/artifact paths must use existing safe resolvers

### 1.2 Never
- Commit/push directly to main
- Delete or skip tests to make them pass
- Open arbitrary paths without safety-zone restriction
- Add dependencies without justification in PR body
- Build something vague: if unclear, STOP and request next instruction

## 2. Pre-Work Ritual (every cycle)
Run these and confirm clean working tree before starting:
- git status
- git branch --show-current
- gh auth status
If dirty, identify cause and git restore / git stash to clean.

## 3. Branch / Commit / PR Rules
### 3.1 Branch naming
prNN-short-slug (e.g. pr54-artifact-open-reveal-safe)
Follow sequential numbering from master-plan.md.

### 3.2 Commit messages
Use: feat(desktop): / fix: / test: / ci: / docs:
One PR = one topic.

### 3.3 PR body template (required)
- Summary (what was done)
- Changes (key points)
- How to test (minimal steps)
- Notes / Risks (compat, safety, known limitations)

## 4. Testing Rules
### 4.1 Minimum local checks
- Rust: cd src-tauri && cargo fmt --check && cargo test
- Front: npm run lint
- If flake suspected: run cargo test at least 2x

### 4.2 Do not break CI
- If CI fails, first identify failing test name and conditions from logs

## 5. Logging (for long-running operation)
At end of each PR cycle, record:
- Commands run
- Changed files (git diff --stat)
- Test results (pass/fail, count)
- PR URL and checks result
Save to: .codex-logs/result-YYYYMMDD-HHMMSS-N.md

## 6. Path Safety
### 6.1 File path safety
run/artifact resolution: catalog name -> safe resolver -> absolute path.
Reject ../ inputs, relative path composition, arbitrary path open.

### 6.2 open/reveal safety zone
Allow only under pipeline_root / out_base_dir.
Reject anything that cannot be canonicalized or is outside safety zone.

## 7. Stop Conditions
Stop and leave a summary if:
- Specs cannot be determined (no evidence found)
- Tests fail repeatedly with unknown cause
- Diff exceeds ~400 lines
- Merge conflicts are unclear
Leave in .codex-logs/: where stopped, failure log summary, what info is needed.

## 8. Development Philosophy
- "Operational reliability" over "UI polish"
- Small increments (split PRs)
- Avoid breaking existing features; advance by addition

## 9. Workflow Orchestration (Boris Cherny method, adapted for Codex)

### 9.1 Plan Before Code
- For ANY task with 3+ steps or architectural decisions: write a plan first
- If something breaks during implementation: STOP and re-plan immediately
- Do not keep pushing broken code hoping to fix it incrementally
- Write detailed specs upfront to reduce ambiguity
- Save plan to: tasks/todo.md with checkable items

### 9.2 Self-Improvement Loop
- After ANY correction, failed test, or unexpected behavior:
  append the pattern to tasks/lessons.md
- Write rules for yourself that prevent the same mistake
- Review tasks/lessons.md at the start of each cycle
- Format: | date | what went wrong | root cause | prevention rule |

### 9.3 Verification Before Done
- Never mark a task complete without proving it works
- Run: cargo test, npm run lint, and any relevant checks
- Diff your changes against main: git diff main --stat
- Ask yourself: "Would a senior engineer approve this PR?"
- If cargo fmt --check fails on YOUR changes (not pre-existing), fix before done

### 9.4 Demand Elegance (Balanced)
- For non-trivial changes: pause and ask "is there a more elegant way?"
- Skip this for simple, obvious one-line fixes
- If a fix feels hacky: reconsider with full context before committing
- Challenge your own work before presenting it

### 9.5 Autonomous Bug Fixing
- When encountering a bug or failing test: just fix it
- Do not ask the user for step-by-step instructions
- Point at logs, errors, failing tests, then resolve them
- Goal: zero context-switching required from the user
- Go fix failing CI tests without being told how

### 9.6 Task Tracking Per Cycle
- Before starting: write plan to tasks/todo.md with checkable items
- Mark items complete as you go: - [x] done
- After each cycle: add a Review section to tasks/todo.md
- After corrections: update tasks/lessons.md
- Keep tasks/todo.md and tasks/lessons.md always up to date

## 10. Core Principles (reinforced)
- Simplicity First: make every change as simple as possible
- No Laziness: find root causes, no temporary band-aid fixes
- Minimal Impact: only touch what is necessary, avoid introducing new bugs
- Senior Engineer Standard: all output should meet staff-level review quality

diff --git a/AGENTS.md b/AGENTS.md
--- a/AGENTS.md
+++ b/AGENTS.md
@@
 ## 1. Agent Rules
 ### 1.1 Always
 - Never guess specs: use rg / existing code / existing tests for evidence
 - Keep diffs small: target 200-400 lines per PR. Split if larger.
 - Preserve backward compat: keep existing Tauri commands and UI flows
 - Respect safety zones: run/artifact paths must use existing safe resolvers
+ - PowerShell-first environment: avoid Bash-only constructs (heredoc/redirect idioms). Use PowerShell here-strings and pipes.

 ### 1.2 Never
 - Commit/push directly to main
 - Delete or skip tests to make them pass
 - Open arbitrary paths without safety-zone restriction
 - Add dependencies without justification in PR body
 - Build something vague: if unclear, STOP and request next instruction
+ - Make broad refactors "for elegance" unless explicitly required by the PR plan (prefer minimal hunk edits).

@@
 ## 9. Workflow Orchestration (Boris Cherny method, adapted for Codex)
@@
 ### 9.4 Demand Elegance (Balanced)
 - For non-trivial changes: pause and ask "is there a more elegant way?"
 - Skip this for simple, obvious one-line fixes
 - If a fix feels hacky: reconsider with full context before committing
 - Challenge your own work before presenting it
+ - Constraint: elegance must not expand scope. Prefer minimal-diff solutions. If refactor is needed, split into a dedicated PR and stop.

 ### 9.5 Autonomous Bug Fixing
 - When encountering a bug or failing test: just fix it
 - Do not ask the user for step-by-step instructions
 - Point at logs, errors, failing tests, then resolve them
 - Goal: zero context-switching required from the user
 - Go fix failing CI tests without being told how
+ - Scope guard: autonomous fixes must stay within the current PR scope. If the fix would broaden scope or exceed ~400 lines, open a dedicated PR (draft is fine) and STOP.

@@
 ## 10. Core Principles (reinforced)
 - Simplicity First: make every change as simple as possible
 - No Laziness: find root causes, no temporary band-aid fixes
 - Minimal Impact: only touch what is necessary, avoid introducing new bugs
 - Senior Engineer Standard: all output should meet staff-level review quality
+ - No local-only completion: any completed work must be pushed and recorded as a GitHub PR (draft allowed). If PR cannot be created, STOP and log why.