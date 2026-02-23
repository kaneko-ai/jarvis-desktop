# Dev SOP

## Per-PR Ritual

Run these before starting any implementation:

```powershell
git status
git branch --show-current
gh auth status
```

Rules:
- Work only from a clean tree.
- If dirty, stash or resolve before editing.
- Keep each PR small and reviewable.

## Branching and Merge Order

1. Merge prerequisite PRs first (dependency order).
2. `git switch main`
3. `git pull --ff-only`
4. Create feature branch from updated `main`.

## Verification Baseline

For desktop changes:

```powershell
npm run lint
cd src-tauri
cargo test
cd ..
```

For flaky-test investigation:
- Repeat `cargo test` in parallel runs.
- Do not force global single-thread mode as a permanent fix.
- Eliminate shared-state collisions instead.

## CI Failure Triage (Fast Path)

1. `gh pr checks <pr-number>`
2. Open failing GitHub Actions run URL.
3. Identify failing test/check and first failing log snippet.
4. Reproduce locally with the same command.
5. Apply minimal fix and rerun local checks.

## Release-Safety Guardrails

- Do not change config precedence (`file -> env -> autodetect`) unless explicitly approved.
- Do not introduce breaking changes to workspace export/import layout.
- Keep fallback paths for log and diagnostics features.
