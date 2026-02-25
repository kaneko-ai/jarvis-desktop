# master-plan.md
# Jarvis Desktop Master Plan (2026-02-24)

## 0. North Star
Port jarvis-ml-pipeline features into jarvis-desktop as a stable desktop app.

## Completed PRs
- PR29-PR46: foundation UI, validation, runbook, SOP
- PR54: safe artifact open/reveal/copy path actions
- PR55: artifact preview renderers
- PR56: artifact search, badges, size display
- PR57: compare runs artifact diff
- PR58: template validation UI parity
- PR59: template validation backend contract
- PR60: validate pipeline steps UI adoption
- PR61: analyze pipeline start-to-logs smoke test
- PR62: CI UI smoke PR gate
- PR63: CI RunDiagStrict smoke PR gate

## Next Tasks (priority order)
- PR64: dashboard statistics widget (run count, success rate, avg duration)
- PR65: log viewer filtering and search
- PR66: settings export/import functionality
- PR67: E2E test suite expansion (Playwright)
- PR68: performance optimization (startup time, large run lists)
- PR69: error recovery and retry mechanisms
- PR70: user preferences persistence
