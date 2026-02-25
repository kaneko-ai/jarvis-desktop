# Annotated Review: pr1-auto-improvement Plan

## Step-by-step annotations

1. `pr1-auto-improvement`
- [WARN] Naming is close to the repo convention (`prNN-short-slug`) but lacks clarity on what the single logical change is. As written, this can still permit broad scope.

2. `chore: automated improvement cycle 1`
- [FIX] Too generic for a one-topic PR. It does not define concrete deliverable boundaries, acceptance criteria, or which subsystem is affected.

3. `Implement improvements.`
- [FIX] Unbounded scope. This directly conflicts with constraints: one logical change per PR, max 400 lines, max 12 files, and no unrelated changes.

## Constraint coverage check

- [WARN] No step enforces the 400-line / 12-file budget during execution.
- [WARN] No explicit check for forbidden patterns in newly added code (`console.log`, `TODO`, `FIXME`, `HACK`).
- [WARN] No verification step for required quality gates (`cargo fmt --check`, tests, lint).
- [WARN] No explicit requirement to keep PR title/body in English and comments in English.

## Corrected Plan (required because [FIX] exists)

1. Define one concrete improvement target and freeze scope
- Decide exactly one logical change (example: tighten artifact path canonicalization for `open/reveal` command only).
- Write in-scope and out-of-scope bullets before coding.
- Reject any extra refactor not required to complete that target.

2. Create branch and PR metadata aligned to conventions
- Branch: `prNN-<short-slug>` based on `master-plan.md` sequence.
- Prepare English PR title/body with required sections (Summary, Changes, How to test, Notes/Risks).

3. Implement minimal-diff change only
- Touch only files needed for the chosen target.
- Keep edits under `<= 400` changed lines and `<= 12` changed files.
- Avoid forbidden new patterns: `console.log`, `TODO`, `FIXME`, `HACK`.
- Preserve backward compatibility of existing Tauri commands/UI flows.

4. Add or update focused tests for the exact behavior changed
- Cover happy path and at least one edge/failure case relevant to the target.
- Do not modify unrelated tests.

5. Run mandatory validation gates
- `cd src-tauri && cargo fmt --check`
- `cd src-tauri && cargo test` (rerun once if flake suspected)
- `npm run lint`
- Fail the cycle if any gate fails.

6. Enforce diff hygiene and language constraints before PR
- Confirm `git diff --stat` remains within line/file limits.
- Confirm newly added comments are English.
- Confirm PR title/body are English and scope remains single-topic.

7. Record cycle log and open PR
- Save run log to `.codex-logs/result-YYYYMMDD-HHMMSS-N.md` with commands, diff stat, and test results.
- Push branch and open GitHub PR (draft allowed).
- Stop and log reason if PR cannot be created.
