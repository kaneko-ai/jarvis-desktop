import { expect, test } from "@playwright/test";

const PIPELINE_RUNS = [
  {
    run_id: "run-20260224-0001",
    canonical_id: "arxiv:1706.03762",
    template_id: "TEMPLATE_TREE",
    created_at: "2026-02-24T10:00:00Z",
    status: "success",
    run_dir: "C:\\mock\\logs\\runs\\run-20260224-0001",
  },
  {
    run_id: "run-20260224-0002",
    canonical_id: "arxiv:2401.00001",
    template_id: "TEMPLATE_RELATED",
    created_at: "2026-02-24T10:05:00Z",
    status: "needs_retry",
    run_dir: "C:\\mock\\logs\\runs\\run-20260224-0002",
  },
];

const RUN_TEXT_BY_KIND = {
  input: "INFO boot start\nINFO resolve config\nWARN rate limit\nINFO done",
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript(({ pipelineRuns, runTextByKind }) => {
    const clone = (value) => JSON.parse(JSON.stringify(value));

    const invoke = async (command, args) => {
      switch (command) {
        case "get_runtime_config":
        case "reload_runtime_config":
          return {
            ok: true,
            message: "ok",
            pipeline_root: "C:\\mock\\pipeline",
            out_base_dir: "C:\\mock\\pipeline\\logs\\runs",
          };
        case "preflight_check":
          return {
            ok: true,
            checks: [
              { name: "pipeline_root", ok: true, detail: "ok" },
              { name: "out_dir", ok: true, detail: "ok" },
              { name: "python", ok: true, detail: "ok" },
              { name: "pipeline_markers", ok: true, detail: "ok" },
            ],
          };
        case "list_task_templates":
          return [
            {
              id: "TEMPLATE_TREE",
              label: "Tree",
              params: [
                { key: "depth", default_value: 1 },
                { key: "max_per_level", default_value: 5 },
              ],
              required_fields: [],
            },
          ];
        case "list_runs":
          return [];
        case "list_pipeline_runs":
          return clone(pipelineRuns);
        case "get_run_dashboard_stats":
          return {
            total_runs: 2,
            success_rate_pct: 50,
            avg_duration_sec: 12.5,
            duration_sample_count: 2,
          };
        case "read_run_text_tail": {
          const kind = String(args?.kind ?? "");
          const content = runTextByKind[kind] ?? "";
          return { content, truncated: false };
        }
        case "read_run_text": {
          const kind = String(args?.kind ?? "");
          return runTextByKind[kind] ?? "";
        }
        case "list_jobs":
        case "list_pipelines":
        case "list_diagnostics":
        case "list_workspace_exports":
        case "list_workspace_imports":
        case "library_list":
        case "library_search":
        case "list_run_artifacts":
          return [];
        case "library_stats":
          return { total: 0, by_status: {}, by_kind: {} };
        case "library_get":
          return null;
        case "get_settings":
          return {
            auto_retry_enabled: true,
            auto_retry_max_per_job: 2,
            auto_retry_max_per_pipeline: 3,
            auto_retry_base_delay_seconds: 5,
            auto_retry_max_delay_seconds: 60,
          };
        case "get_pipeline_repo_status":
          return {
            remote_url: "https://example.invalid/mock.git",
            local_path: "C:\\mock\\pipeline",
            git_ref: "main",
            head_commit: "mock",
            last_sync_at: "2026-02-24T10:00:00Z",
          };
        case "tick_auto_retry":
          return { reason: "idle", acted: false };
        case "normalize_identifier": {
          const input = String(args?.input ?? "");
          return {
            kind: "arxiv",
            canonical: input,
            display: input,
            warnings: [],
            errors: [],
          };
        }
        case "parse_graph_json":
          return { nodes: [], edges: [] };
        default:
          return null;
      }
    };

    window.__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback) => callback,
    };
  }, { pipelineRuns: PIPELINE_RUNS, runTextByKind: RUN_TEXT_BY_KIND });

  await page.goto("/");
});

test("runs explorer applies live-log search and matches-only filtering", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Javis Desktop" })).toBeVisible();

  await page.getByRole("button", { name: "Runs" }).click();
  await expect(page.getByRole("heading", { name: "Run Explorer" })).toBeVisible();

  await expect(page.getByText("run-20260224-0001").first()).toBeVisible();
  await expect(page.locator("body")).toContainText("run_id: run-20260224-0001");

  const liveLogPanel = page.locator("pre").first();
  await expect(liveLogPanel).toContainText("INFO boot start");
  await expect(page.getByText("matches=4 / lines=4")).toBeVisible();

  const searchInput = page.getByPlaceholder("search logs");
  await searchInput.fill("warn");
  await expect(page.getByText("matches=1 / lines=4")).toBeVisible();

  await page.getByLabel("Matches only").check();
  await expect(liveLogPanel).toContainText("WARN rate limit");
  await expect(liveLogPanel).not.toContainText("INFO boot start");

  await searchInput.fill("no-such-token");
  await expect(page.getByText("No log lines matched the current search.")).toBeVisible();
});
