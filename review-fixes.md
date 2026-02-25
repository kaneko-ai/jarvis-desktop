# Review Fixes

## Findings
1. **Diff hygiene / scope drift**: The PR title targets run explorer/dashboard data source unification, but the diff also included `auto-dev.ps1`, `master-plan.md`, `tasks/lessons.md`, and `tasks/todo.md` changes unrelated to runtime run-root behavior.
2. **Constraint risk**: `git diff --shortstat main...HEAD` showed `232 insertions + 171 deletions = 403 changed lines`, which exceeds the AGENTS target envelope (~200-400) and likely exceeds `MaxDiffLines=400`.
3. **Formatting artifact**: `master-plan.md` contained a UTF-8 BOM at line 1 (`EF BB BF`), creating an unnecessary formatting-only hunk.

## Fixes Applied
- Reverted unrelated files in the working tree to `main`:
  - `auto-dev.ps1`
  - `master-plan.md`
  - `tasks/lessons.md`
  - `tasks/todo.md`
- Kept the feature implementation in `src-tauri/src/main.rs` intact.

## Verification
- `cargo fmt --check --manifest-path src-tauri/Cargo.toml` passed.
- `cargo test --manifest-path src-tauri/Cargo.toml -q` passed (80 tests).
- `npm run lint --silent` passed with 0 errors (9 existing warnings in `src/App.jsx`).

## Note
These fixes are currently in the **working tree**. To make `main...HEAD` reflect this cleaned PR scope, commit the reverts.
