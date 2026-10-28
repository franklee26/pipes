import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { desktopApi, displayError } from "./api";
import type { ApprovePromotion, PromotionContext, RecordConsumerDeployment, RecordHarnessResult, StartPipelineRun } from "./types";

const deploymentRequest: RecordConsumerDeployment = {
  appId: "atlas", stageId: "beta", consumerId: "personal-mac",
  expectedManifestHash: "a".repeat(64), expectedUpdatedAt: "2026-10-26T12:00:00Z",
  releaseRef: "v1.5.0", notes: null, approved: true,
};
const promotionContext: PromotionContext = {
  appId: "atlas", stageId: "beta", releaseRef: "v1.5.0",
  expectedManifestHash: "a".repeat(64), expectedUpdatedAt: "2026-10-26T12:00:00Z",
};
const harnessRequest: RecordHarnessResult = { ...promotionContext, checkId: "unit", total: 50, passed: 45, notes: null };
const approvalRequest: ApprovePromotion = { ...promotionContext, expectedAssessmentHash: "b".repeat(64), approved: true, acknowledgeOptional: true, notes: null };
const runRequest: StartPipelineRun = { appId: "atlas", expectedManifestHash: "a".repeat(64), expectedUpdatedAt: "2026-10-26T12:00:00Z", releaseRef: "v1.5.0", approved: true };

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => true) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

beforeEach(() => {
  vi.mocked(isTauri).mockReturnValue(true);
  vi.mocked(invoke).mockResolvedValue(undefined);
});

describe("native bridge", () => {
  it("uses the Rust command names and camelCase registration arguments", async () => {
    await desktopApi.registryInfo();
    await desktopApi.listApps();
    await desktopApi.previewManifest("/project/pipes.yaml");
    await desktopApi.registerManifest("/project/pipes.yaml", "reviewed-hash", true);
    await desktopApi.unregisterApp("atlas");
    expect(invoke).toHaveBeenNthCalledWith(1, "registry_info", undefined);
    expect(invoke).toHaveBeenNthCalledWith(2, "list_apps", undefined);
    expect(invoke).toHaveBeenNthCalledWith(3, "preview_manifest", { path: "/project/pipes.yaml" });
    expect(invoke).toHaveBeenNthCalledWith(4, "register_manifest", {
      path: "/project/pipes.yaml", expectedHash: "reviewed-hash", replace: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(5, "unregister_app", { appId: "atlas" });
  });

  it("preserves backend errors", async () => {
    const error = { code: "manifest_changed", message: "Review the changed file." };
    vi.mocked(invoke).mockRejectedValueOnce(error);
    await expect(desktopApi.registerManifest("/project/pipes.yaml", "old", false)).rejects.toEqual(error);
    expect(displayError(error)).toEqual(error);
  });

  it("uses the consumer command names and sends the complete reviewed approval request", async () => {
    await desktopApi.listConsumerDeployments();
    await desktopApi.listConsumerDeployments("atlas");
    await desktopApi.recordConsumerDeployment(deploymentRequest);
    expect(invoke).toHaveBeenNthCalledWith(1, "list_consumer_deployments", { appId: null });
    expect(invoke).toHaveBeenNthCalledWith(2, "list_consumer_deployments", { appId: "atlas" });
    expect(invoke).toHaveBeenNthCalledWith(3, "record_consumer_deployment", { request: deploymentRequest });
  });

  it("preserves stale registration errors from consumer recording", async () => {
    const error = { code: "registration_changed", message: "Review the current destination." };
    vi.mocked(invoke).mockRejectedValueOnce(error);
    await expect(desktopApi.recordConsumerDeployment(deploymentRequest)).rejects.toEqual(error);
  });

  it("forwards the exact reporting, readiness and approval contracts to native handlers", async () => {
    await desktopApi.listHarnessResults();
    await desktopApi.listPromotionApprovals("atlas");
    await desktopApi.recordHarnessResult(harnessRequest);
    await desktopApi.assessPromotion(promotionContext);
    await desktopApi.approvePromotion(approvalRequest);
    expect(invoke).toHaveBeenNthCalledWith(1, "list_harness_results", { appId: null });
    expect(invoke).toHaveBeenNthCalledWith(2, "list_promotion_approvals", { appId: "atlas" });
    expect(invoke).toHaveBeenNthCalledWith(3, "record_harness_result", { request: harnessRequest });
    expect(invoke).toHaveBeenNthCalledWith(4, "assess_promotion", { request: promotionContext });
    expect(invoke).toHaveBeenNthCalledWith(5, "approve_promotion", { request: approvalRequest });
  });

  it("requires the native runtime instead of returning sample data", async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(desktopApi.listApps()).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.listConsumerDeployments()).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.recordConsumerDeployment(deploymentRequest)).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.listHarnessResults()).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.listPromotionApprovals()).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.recordHarnessResult(harnessRequest)).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.assessPromotion(promotionContext)).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.approvePromotion(approvalRequest)).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.listPipelineRuns()).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.startPipelineRun(runRequest)).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.getPipelineRun("run-1")).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.pipelineRunLogs("run-1", 0)).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.cancelPipelineRun("run-1")).rejects.toMatchObject({ code: "desktop_required" });
    await expect(desktopApi.approvePipelineStage("run-1", "prod", true)).rejects.toMatchObject({ code: "desktop_required" });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("forwards exact pipeline run arguments and opaque log cursors to the native runner", async () => {
    await desktopApi.listPipelineRuns();
    await desktopApi.listPipelineRuns("atlas");
    await desktopApi.startPipelineRun(runRequest);
    await desktopApi.getPipelineRun("run-1");
    await desktopApi.pipelineRunLogs("run-1", 42);
    await desktopApi.cancelPipelineRun("run-1");
    await desktopApi.approvePipelineStage("run-1", "prod", true);
    await desktopApi.approvePipelineStage("run-1", "prod", false);
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["list_pipeline_runs", { appId: null }],
      ["list_pipeline_runs", { appId: "atlas" }],
      ["start_pipeline_run", { request: runRequest }],
      ["get_pipeline_run", { runId: "run-1" }],
      ["pipeline_run_logs", { runId: "run-1", after: 42 }],
      ["cancel_pipeline_run", { runId: "run-1" }],
      ["approve_pipeline_stage", { runId: "run-1", stageId: "prod", approved: true }],
      ["approve_pipeline_stage", { runId: "run-1", stageId: "prod", approved: false }],
    ]);
  });

  it("preserves stale run-start and stage-approval errors", async () => {
    const stale = { code: "registration_changed", message: "Review the current registration." };
    vi.mocked(invoke).mockRejectedValueOnce(stale);
    await expect(desktopApi.startPipelineRun(runRequest)).rejects.toEqual(stale);
    const conflict = { code: "run_conflict", message: "Stage is not waiting for approval." };
    vi.mocked(invoke).mockRejectedValueOnce(conflict);
    await expect(desktopApi.approvePipelineStage("run-1", "prod", true)).rejects.toEqual(conflict);
  });
  it("treats a cancelled native file picker as cancellation", async () => {
    vi.mocked(open).mockResolvedValueOnce(null);
    await expect(desktopApi.chooseManifest()).resolves.toBeNull();
    expect(open).toHaveBeenCalledWith(expect.objectContaining({
      multiple: false, directory: false,
      filters: [{ name: "YAML manifests", extensions: ["yaml", "yml"] }],
    }));
  });

  it("keeps unexpected errors visible", () => {
    expect(displayError(new Error("IPC disconnected"))).toEqual({
      code: "unexpected", message: "IPC disconnected",
    });
    expect(displayError("unavailable")).toEqual({ code: "unexpected", message: "unavailable" });
  });
});
