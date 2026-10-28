import { vi } from "vitest";
import { reviewPromotion, promotionReady } from "../lib/promotion";
import { simulatePromotionAssessment } from "../lib/promotionPreview";
import { isRunActive, PIPELINE_RUN_HISTORY_LIMIT, summarizeRun } from "../lib/pipelineRuns";
import type { ApprovePromotion, CheckPlanNode, CommandSummary, ConsumerDeployment, ConsumerSummary, HarnessResult, ManifestPreview, PipelineRun, PipesApi, PromotionApproval, PromotionContext, RecordConsumerDeployment, RecordHarnessResult, RegisteredApp, RegistryInfo, RunLogEntry, RunLogPage, StartPipelineRun } from "../lib/types";

export const unitCommandFixture: CommandSummary = {
  id: "unit", exec: ["cargo", "test"], cwd: null, timeout: null, env: {}, secrets: {},
};

export const manifestFixture: ManifestPreview = {
  appId: "atlas",
  name: "Atlas",
  manifestPath: "/projects/atlas/pipes.yaml",
  sourcePath: "/projects/atlas",
  manifestHash: "a".repeat(64),
  manifestYaml: "api: pipes/v1\napp: atlas\n",
  versionFrom: "VERSION",
  execution: { os: "macos", arch: "arm64", timeout: "20m", maxParallel: "auto" },
  prepare: [],
  outputs: [],
  stages: [
    {
      id: "test",
      after: [],
      checks: [unitCommandFixture],
      checkPlan: [{ kind: "command", command: unitCommandFixture }],
      requires: ["checks.passed"],
      requiresApproval: false,
      consumers: [],
      promotionCriteria: [],
    },
    { id: "beta", after: ["test"], checks: [], checkPlan: [], requires: ["previous.passed"], requiresApproval: false, consumers: [], promotionCriteria: [] },
    { id: "gamma", after: ["beta"], checks: [], checkPlan: [], requires: ["previous.passed", "approval.local"], requiresApproval: true, consumers: [], promotionCriteria: [] },
    { id: "prod", after: ["gamma"], checks: [], checkPlan: [], requires: ["previous.passed", "approval.local"], requiresApproval: true, consumers: [], promotionCriteria: [] },
  ],
};

export const registeredFixture: RegisteredApp = {
  manifest: manifestFixture,
  registeredAt: "2026-10-26T12:00:00Z",
  updatedAt: "2026-10-26T12:00:00Z",
};

export const consumerFixture: ConsumerSummary = {
  id: "personal-mac",
  name: "Personal MacBook",
  target: "My MacBook / Applications",
  approval: "manual",
};

export const consumerManifestFixture: ManifestPreview = {
  ...manifestFixture,
  stages: manifestFixture.stages.map((stage) => stage.id === "beta"
    ? { ...stage, consumers: [consumerFixture] }
    : stage),
};

export const consumerRegisteredFixture: RegisteredApp = {
  ...registeredFixture,
  manifest: consumerManifestFixture,
};

export const consumerDeploymentFixture: ConsumerDeployment = {
  id: "1",
  appId: "atlas",
  appName: "Atlas",
  stageId: "beta",
  consumer: consumerFixture,
  releaseRef: "v1.5.0",
  notes: "Copied the beta build to my laptop.",
  manifestHash: manifestFixture.manifestHash,
  manifestPath: manifestFixture.manifestPath,
  sourcePath: manifestFixture.sourcePath,
  registrationUpdatedAt: registeredFixture.updatedAt,
  recordedAt: "2026-10-27T17:00:00Z",
};

export const promotionManifestFixture: ManifestPreview = {
  ...consumerManifestFixture,
  stages: consumerManifestFixture.stages.map((stage) => stage.id === "beta" ? {
    ...stage,
    checks: [unitCommandFixture],
    checkPlan: [{ kind: "command", command: unitCommandFixture }],
    promotionCriteria: [
      { kind: "harness", id: "unit-threshold", checkId: "unit", expectedTests: 50, minimumPassed: 45 },
      { kind: "deployment", id: "personal-delivery", consumerId: consumerFixture.id, requirement: "optional" },
    ],
  } : stage),
};

export const promotionRegisteredFixture: RegisteredApp = { ...registeredFixture, manifest: promotionManifestFixture };

export const harnessResultFixture: HarnessResult = {
  id: "1", appId: "atlas", appName: "Atlas", stageId: "beta", checkId: "unit",
  releaseRef: "v1.5.0", total: 50, passed: 45, notes: "Reported by the external test harness.",
  manifestHash: manifestFixture.manifestHash, manifestPath: manifestFixture.manifestPath,
  sourcePath: manifestFixture.sourcePath, registrationUpdatedAt: registeredFixture.updatedAt,
  recordedAt: "2026-10-27T20:00:00Z",
};

export const infoFixture: RegistryInfo = {
  dataDir: "/state/pipes",
  databasePath: "/state/pipes/registry.sqlite3",
  appVersion: "0.1.0",
  os: "macos",
  arch: "aarch64",
};

export const forkManifestFixture: ManifestPreview = {
  ...manifestFixture,
  stages: [
    { id: "test", after: [], checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [] },
    { id: "linux", after: ["test"], checks: [], checkPlan: [], requires: ["previous.passed"], requiresApproval: false, consumers: [], promotionCriteria: [] },
    { id: "macos", after: ["test"], checks: [], checkPlan: [], requires: ["previous.passed"], requiresApproval: false, consumers: [], promotionCriteria: [] },
    { id: "docs", after: ["test"], checks: [], checkPlan: [], requires: ["previous.passed"], requiresApproval: false, consumers: [], promotionCriteria: [] },
    { id: "prod", after: ["linux", "macos"], checks: [], checkPlan: [], requires: ["previous.passed", "approval.local"], requiresApproval: true, consumers: [], promotionCriteria: [] },
  ],
};

const checkCommands: CommandSummary[] = [
  { id: "lint", exec: ["npm", "run", "check"], cwd: null, timeout: null, env: {}, secrets: {} },
  { id: "rust-unit", exec: ["cargo", "test"], cwd: null, timeout: null, env: {}, secrets: {} },
  { id: "ui", exec: ["npm", "test"], cwd: null, timeout: null, env: {}, secrets: {} },
  { id: "build", exec: ["npm", "run", "build:web"], cwd: null, timeout: null, env: {}, secrets: {} },
  { id: "inspect", exec: ["pipes", "validate", "pipes.yaml"], cwd: null, timeout: null, env: {}, secrets: {} },
];

function leaf(id: string): CheckPlanNode {
  const command = checkCommands.find((item) => item.id === id);
  if (!command) throw new Error(`Missing command fixture: ${id}`);
  return { kind: "command", command };
}

export const composedPlanFixture: CheckPlanNode[] = [
  { kind: "parallel", id: "quick-tests", steps: [leaf("lint"), leaf("rust-unit")], maxParallel: null },
  {
    kind: "parallel",
    id: "acceptance",
    maxParallel: 2,
    steps: [
      leaf("ui"),
      { kind: "sequence", id: "package-checks", steps: [leaf("build"), leaf("inspect")] },
    ],
  },
];

export const composedManifestFixture: ManifestPreview = {
  ...manifestFixture,
  execution: { ...manifestFixture.execution, maxParallel: 8 },
  stages: [
    {
      id: "beta", after: [], checks: checkCommands, checkPlan: composedPlanFixture,
      requires: ["checks.passed"], requiresApproval: false, consumers: [], promotionCriteria: [],
    },
    {
      id: "prod", after: ["beta"], checks: [], checkPlan: [],
      requires: ["previous.passed", "approval.local"], requiresApproval: true, consumers: [], promotionCriteria: [],
    },
  ],
};

export function makeRun(overrides: Partial<PipelineRun> = {}): PipelineRun {
  const manifest = overrides.manifest ?? manifestFixture;
  return {
    id: "run-1",
    appId: manifest.appId,
    appName: manifest.name,
    manifest,
    registrationUpdatedAt: registeredFixture.updatedAt,
    releaseRef: "v1.5.0",
    status: "running",
    startedAt: "2026-10-27T22:00:00Z",
    finishedAt: null,
    maxParallel: 4,
    message: null,
    stages: manifest.stages.map((stage) => ({
      stageId: stage.id, status: "pending", startedAt: null, finishedAt: null, approvedAt: null, message: null,
    })),
    commands: [
      ...manifest.prepare.map((command) => ({ stageId: null, commandId: command.id })),
      ...manifest.stages.flatMap((stage) => stage.checks.map((command) => ({ stageId: stage.id, commandId: command.id }))),
    ].map((command) => ({
      ...command, status: "pending", startedAt: null, finishedAt: null, exitCode: null, message: null,
    })),
    ...overrides,
  };
}

export function makeLog(text: string, sequence = 1, overrides: Partial<RunLogEntry> = {}): RunLogEntry {
  return { sequence, recordedAt: "2026-10-27T22:00:01Z", stageId: "test", commandId: "unit", stream: "stdout", text, ...overrides };
}

export function makeApi(initial: RegisteredApp[] = [], available = true, preview = manifestFixture, initialHistory: ConsumerDeployment[] = [], initialEvidence: { harnessResults?: HarnessResult[]; promotionApprovals?: PromotionApproval[]; runs?: PipelineRun[]; logs?: Record<string, RunLogEntry[]> } = {}) {
  let records = structuredClone(initial);
  let deployments = structuredClone(initialHistory);
  let harnessResults = structuredClone(initialEvidence.harnessResults ?? []);
  let promotionApprovals = structuredClone(initialEvidence.promotionApprovals ?? []);
  let runs = structuredClone(initialEvidence.runs ?? []);
  const logs = structuredClone(initialEvidence.logs ?? {});
  let nextDeploymentId = 1;
  let nextHarnessId = 1;
  let nextApprovalId = 1;
  let nextRunId = 1;
  function getRun(runId: string) {
    const run = runs.find((item) => item.id === runId);
    if (!run) throw { code: "run_not_found", message: "This pipeline run was not found." };
    return run;
  }
  function promotionReview(request: PromotionContext) {
    const app = records.find((item) => item.manifest.appId === request.appId);
    if (!app) throw { code: "not_found", message: "This app is no longer registered." };
    if (app.manifest.manifestHash !== request.expectedManifestHash || app.updatedAt !== request.expectedUpdatedAt) {
      throw { code: "registration_changed", message: "Review the changed registration." };
    }
    const stage = app.manifest.stages.find((item) => item.id === request.stageId);
    if (!stage) throw { code: "stage_not_found", message: "The source stage no longer exists." };
    return reviewPromotion(app, stage);
  }
  return {
    available,
    registryInfo: vi.fn(async () => structuredClone(infoFixture)),
    listApps: vi.fn(async () => structuredClone(records)),
    chooseManifest: vi.fn(async (): Promise<string | null> => preview.manifestPath),
    previewManifest: vi.fn(async (_path: string) => structuredClone(preview)),
    registerManifest: vi.fn(async (path: string, expectedHash: string, _replace: boolean) => {
      const app: RegisteredApp = {
        ...registeredFixture,
        manifest: { ...structuredClone(preview), manifestPath: path, manifestHash: expectedHash },
      };
      records = [...records.filter((record) => record.manifest.appId !== app.manifest.appId), app];
      return structuredClone(app);
    }),
    unregisterApp: vi.fn(async (appId: string) => {
      records = records.filter((record) => record.manifest.appId !== appId);
    }),
    listConsumerDeployments: vi.fn(async (appId?: string) =>
      structuredClone(deployments.filter((record) => appId === undefined || record.appId === appId))),
    recordConsumerDeployment: vi.fn(async (request: RecordConsumerDeployment) => {
      if (!request.approved) throw { code: "approval_required", message: "Manual approval is required." };
      if (!request.releaseRef.trim()) throw { code: "invalid_deployment", message: "Enter a release reference." };
      const app = records.find((item) => item.manifest.appId === request.appId);
      if (!app) throw { code: "not_found", message: "This app is no longer registered." };
      if (app.manifest.manifestHash !== request.expectedManifestHash || app.updatedAt !== request.expectedUpdatedAt) {
        throw { code: "registration_changed", message: "The registration changed. Review it again before recording." };
      }
      const consumer = app.manifest.stages.find((stage) => stage.id === request.stageId)
        ?.consumers.find((item) => item.id === request.consumerId);
      if (!consumer) throw { code: "consumer_not_found", message: "This consumer is no longer declared." };
      while (deployments.some((record) => record.id === String(nextDeploymentId))) nextDeploymentId += 1;
      const record: ConsumerDeployment = {
        id: String(nextDeploymentId++),
        appId: app.manifest.appId,
        appName: app.manifest.name,
        stageId: request.stageId,
        consumer: structuredClone(consumer),
        releaseRef: request.releaseRef.trim(),
        notes: request.notes?.trim() || null,
        manifestHash: app.manifest.manifestHash,
        manifestPath: app.manifest.manifestPath,
        sourcePath: app.manifest.sourcePath,
        registrationUpdatedAt: app.updatedAt,
        recordedAt: "2026-10-27T18:00:00Z",
      };
      deployments = [record, ...deployments];
      return structuredClone(record);
    }),
    listHarnessResults: vi.fn(async (appId?: string) =>
      structuredClone(harnessResults.filter((record) => appId === undefined || record.appId === appId))),
    listPromotionApprovals: vi.fn(async (appId?: string) =>
      structuredClone(promotionApprovals.filter((record) => appId === undefined || record.assessment.appId === appId))),
    recordHarnessResult: vi.fn(async (request: RecordHarnessResult) => {
      const review = promotionReview(request);
      if (!review.stage.checks.some((check) => check.id === request.checkId)) throw { code: "check_not_found", message: "The harness is no longer configured." };
      if (!request.releaseRef.trim() || !Number.isInteger(request.total) || !Number.isInteger(request.passed) || request.passed < 0 || request.total < request.passed || request.total > 4294967295) {
        throw { code: "invalid_harness_result", message: "Enter valid observed counts and a reference." };
      }
      while (harnessResults.some((result) => result.id === String(nextHarnessId))) nextHarnessId += 1;
      const result: HarnessResult = {
        id: String(nextHarnessId++), appId: review.appId, appName: review.appName, stageId: review.stage.id,
        checkId: request.checkId, releaseRef: request.releaseRef.trim(), total: request.total, passed: request.passed,
        notes: request.notes?.trim() || null, manifestHash: review.manifestHash, manifestPath: review.manifestPath,
        sourcePath: review.sourcePath, registrationUpdatedAt: review.updatedAt, recordedAt: "2026-10-27T21:00:00Z",
      };
      harnessResults = [result, ...harnessResults];
      return structuredClone(result);
    }),
    assessPromotion: vi.fn(async (request: PromotionContext) =>
      simulatePromotionAssessment(promotionReview(request), request.releaseRef, harnessResults, deployments)),
    approvePromotion: vi.fn(async (request: ApprovePromotion) => {
      if (!request.approved) throw { code: "approval_required", message: "Explicit approval is required." };
      const assessment = simulatePromotionAssessment(promotionReview(request), request.releaseRef, harnessResults, deployments);
      if (assessment.assessmentHash !== request.expectedAssessmentHash) throw { code: "assessment_changed", message: "Evidence changed. Assess readiness again." };
      if (!promotionReady(assessment)) throw { code: "promotion_blocked", message: "Required promotion criteria are not satisfied." };
      const skippedOptional = assessment.criteria.filter((criterion) => criterion.status === "warning").map((criterion) => criterion.criterion.id);
      if (skippedOptional.length && !request.acknowledgeOptional) throw { code: "optional_steps_unacknowledged", message: "Acknowledge the skipped optional steps." };
      while (promotionApprovals.some((approval) => approval.id === String(nextApprovalId))) nextApprovalId += 1;
      const approval: PromotionApproval = {
        id: String(nextApprovalId++), assessment: structuredClone(assessment), skippedOptional,
        notes: request.notes?.trim() || null, recordedAt: "2026-10-27T21:05:00Z",
      };
      promotionApprovals = [approval, ...promotionApprovals];
      return structuredClone(approval);
    }),
    listPipelineRuns: vi.fn(async (appId?: string) =>
      structuredClone(runs.filter((run) => appId === undefined || run.appId === appId).slice(0, PIPELINE_RUN_HISTORY_LIMIT).map(summarizeRun))),
    startPipelineRun: vi.fn(async (request: StartPipelineRun) => {
      if (!request.approved) throw { code: "run_approval_required", message: "Explicit execution approval is required." };
      if (!request.releaseRef.trim()) throw { code: "invalid_run", message: "Enter a release reference." };
      const app = records.find((item) => item.manifest.appId === request.appId);
      if (!app) throw { code: "not_found", message: "This app is no longer registered." };
      if (app.manifest.manifestHash !== request.expectedManifestHash || app.updatedAt !== request.expectedUpdatedAt) {
        throw { code: "registration_changed", message: "Review the changed registration before running." };
      }
      while (runs.some((run) => run.id === `run-${nextRunId}`)) nextRunId += 1;
      const run = makeRun({
        id: `run-${nextRunId++}`, manifest: structuredClone(app.manifest),
        registrationUpdatedAt: app.updatedAt, releaseRef: request.releaseRef.trim(),
      });
      runs = [run, ...runs];
      return structuredClone(run);
    }),
    getPipelineRun: vi.fn(async (runId: string) => structuredClone(getRun(runId))),
    pipelineRunLogs: vi.fn(async (runId: string, after: number): Promise<RunLogPage> => {
      getRun(runId);
      const remaining = (logs[runId] ?? []).filter((entry) => entry.sequence > after);
      const entries = remaining.slice(0, 200);
      return structuredClone({ entries, nextCursor: entries.at(-1)?.sequence ?? after, hasMore: remaining.length > entries.length });
    }),
    cancelPipelineRun: vi.fn(async (runId: string) => {
      const run = getRun(runId);
      if (!isRunActive(run.status)) throw { code: "run_conflict", message: "The run has already finished." };
      run.status = "cancelled";
      run.finishedAt = "2026-10-27T22:01:00Z";
      run.message = "Cancelled by user.";
      run.stages = run.stages.map((stage) => isRunActive(stage.status) ? { ...stage, status: "cancelled" } : stage);
      run.commands = run.commands.map((command) => isRunActive(command.status) ? { ...command, status: "cancelled" } : command);
      return structuredClone(run);
    }),
    approvePipelineStage: vi.fn(async (runId: string, stageId: string, approved: boolean) => {
      const run = getRun(runId);
      if (!approved) throw { code: "run_approval_required", message: "Explicit stage approval is required." };
      const stage = run.stages.find((item) => item.stageId === stageId);
      if (!isRunActive(run.status) || stage?.status !== "waiting_approval" || stage.approvedAt ||
        !run.manifest.stages.find((item) => item.id === stageId)?.requiresApproval) {
        throw { code: "run_conflict", message: "This stage is not waiting for local approval." };
      }
      stage.approvedAt = "2026-10-27T22:01:00Z";
      stage.status = "running";
      run.status = "running";
      return structuredClone(run);
    }),
  } satisfies PipesApi;
}
