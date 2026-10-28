import { invoke, isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { ConsumerDeployment, HarnessResult, ManifestPreview, PipelineRun, PipelineRunSummary, PipesApi, PromotionApproval, PromotionAssessment, RegisteredApp, RegistryInfo, RunLogPage } from "./types";

export interface DisplayError {
  code: string;
  message: string;
}

class DesktopRequiredError extends Error {
  readonly code = "desktop_required";
}

export function displayError(error: unknown): DisplayError {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof error.code === "string" &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return { code: error.code, message: error.message };
  }
  return {
    code: "unexpected",
    message: error instanceof Error ? error.message : String(error),
  };
}

function requireDesktop(): void {
  if (!isTauri()) {
    throw new DesktopRequiredError("Open the Pipes desktop app to access the local registry.");
  }
}

async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  requireDesktop();
  return invoke<T>(command, args);
}

export const desktopApi: PipesApi = {
  available: isTauri(),
  registryInfo: () => call<RegistryInfo>("registry_info"),
  listApps: () => call<RegisteredApp[]>("list_apps"),
  async chooseManifest() {
    requireDesktop();
    const path = await open({
      title: "Register a Pipes manifest",
      multiple: false,
      directory: false,
      filters: [{ name: "YAML manifests", extensions: ["yaml", "yml"] }],
    });
    if (path !== null && typeof path !== "string") {
      throw new Error("The file picker returned more than one manifest.");
    }
    return path;
  },
  previewManifest: (path) =>
    call<ManifestPreview>("preview_manifest", { path }),
  registerManifest: (path, expectedHash, replace) =>
    call<RegisteredApp>("register_manifest", { path, expectedHash, replace }),
  unregisterApp: (appId) => call<void>("unregister_app", { appId }),
  listConsumerDeployments: (appId) =>
    call<ConsumerDeployment[]>("list_consumer_deployments", { appId: appId ?? null }),
  recordConsumerDeployment: (request) =>
    call<ConsumerDeployment>("record_consumer_deployment", { request }),
  listHarnessResults: (appId) =>
    call<HarnessResult[]>("list_harness_results", { appId: appId ?? null }),
  recordHarnessResult: (request) =>
    call<HarnessResult>("record_harness_result", { request }),
  assessPromotion: (request) =>
    call<PromotionAssessment>("assess_promotion", { request }),
  approvePromotion: (request) =>
    call<PromotionApproval>("approve_promotion", { request }),
  listPromotionApprovals: (appId) =>
    call<PromotionApproval[]>("list_promotion_approvals", { appId: appId ?? null }),
  listPipelineRuns: (appId) =>
    call<PipelineRunSummary[]>("list_pipeline_runs", { appId: appId ?? null }),
  startPipelineRun: (request) =>
    call<PipelineRun>("start_pipeline_run", { request }),
  getPipelineRun: (runId) =>
    call<PipelineRun>("get_pipeline_run", { runId }),
  pipelineRunLogs: (runId, after) =>
    call<RunLogPage>("pipeline_run_logs", { runId, after }),
  cancelPipelineRun: (runId) =>
    call<PipelineRun>("cancel_pipeline_run", { runId }),
  approvePipelineStage: (runId, stageId, approved) =>
    call<PipelineRun>("approve_pipeline_stage", { runId, stageId, approved }),
};
