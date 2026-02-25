# Task Tracker
<!-- Codex updates this file at the start and end of each cycle -->

## Current Cycle
- [x] PR67: run pre-work hygiene checks and create/switch `pr67-e2e-suite-expansion`
- [x] PR67: add Playwright toolchain (`@playwright/test`), scripts, and deterministic config
- [x] PR67: add critical desktop-flow E2E spec for Runs screen behavior
- [x] PR67: wire PR-gated Playwright workflow with failure artifact upload
- [x] PR67: update RUNBOOK E2E command note and complete required verification checks
- [x] PR67: commit with required message and record cycle log artifacts
- [x] PR68: pre-work gate complete (`git status`, `git branch --show-current`, `gh auth status`) and dirty tree stashed
- [x] PR68: finish validation/logging/PR handoff (`git diff --stat` <= 400 lines, <= 12 files)

## Review (Current Cycle)
- Added Playwright dependency and scripts in `package.json`: `test:e2e`, `test:e2e:headed`.
- Added deterministic Playwright config (`playwright.config.ts`) with single-worker Chromium, local Vite web server, and failure artifacts (trace/screenshot).
- Added `tests/e2e/desktop-critical-flow.spec.ts` covering critical Runs flow: list visibility, selected run details, log search, and matches-only filtering with mocked Tauri `invoke`.
- Added PR-gated workflow `.github/workflows/playwright-e2e.yml` to run E2E on `pull_request`/`workflow_dispatch` and upload Playwright artifacts.
- Updated `RUNBOOK.md` with local Playwright E2E commands and artifact paths.
- Verification: `npm run lint` passed with existing 9 warnings in `src/App.jsx`; `cargo test` passed (78 tests); `cargo fmt --check` failed due pre-existing formatting drift in `src-tauri/src/main.rs`; `npm run test:e2e` passed (1 test).
- PR68 (in progress): unified pipeline run read roots (`out_base_dir` primary + legacy fallback), refactored run listing/stats/read/open resolution path, and added focused fallback/overlap tests.
- PR68 verification: `cargo fmt --check` passed; `cargo test` passed (80 tests); `npm ci && npm run build` passed; `npm run lint` passed with 0 errors (9 existing warnings).

## Completed
- [x] PR54: safe artifact open/reveal/copy path actions
- [x] PR55: artifact preview renderers
- [x] PR56: artifact search, badges, size display
- [x] PR57: compare runs artifact diff
- [x] PR58: template validation UI parity
- [x] PR59: template validation backend contract
- [x] PR60: validate pipeline steps UI adoption
- [x] PR61: analyze pipeline start-to-logs smoke test
- [x] PR62: CI UI smoke PR gate
- [x] PR63: CI RunDiagStrict smoke PR gate