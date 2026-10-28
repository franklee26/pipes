export type CommandResult =
  | { type: "exitCode"; successCodes: number[] }
  | { type: "jsonFile"; path: string; pointer: string; success: string | number | boolean; failure: string | number | boolean };

export interface CommandSummary {
  id: string;
  exec: string[];
  cwd: string | null;
  timeout: string | null;
  env: Record<string, string>;
  secrets: Record<string, string>;
  result?: CommandResult;
}

export type Parallelism = "auto" | number;

export interface ConsumerSummary {
  id: string;
  name: string;
  target: string;
  approval: "manual";
}

export type PromotionCriterion =
  | { kind: "harness"; id: string; checkId: string; expectedTests: number; minimumPassed: number }
  | { kind: "deployment"; id: string; consumerId: string; requirement: "required" | "optional" };

export type CheckPlanNode =
  | { kind: "command"; command: CommandSummary }
  | { kind: "sequence"; id: string; steps: CheckPlanNode[] }
  | { kind: "parallel"; id: string; steps: CheckPlanNode[]; maxParallel: number | null };

export interface StageSummary {
  id: string;
  after: string[];
  /** Inspection-only flattened leaves; checkPlan preserves ordering and barriers. */
  checks: CommandSummary[];
  checkPlan: CheckPlanNode[];
  requires: string[];
  requiresApproval: boolean;
  consumers: ConsumerSummary[];
  promotionCriteria: PromotionCriterion[];
}

export interface ManifestPreview {
  appId: string;
  name: string;
  manifestPath: string;
  sourcePath: string;
  manifestHash: string;
  manifestYaml: string;
  versionFrom: string;
  execution: {
    os: string | null;
    arch: string | null;
    timeout: string | null;
    maxParallel: Parallelism;
  };
  stages: StageSummary[];
  prepare: CommandSummary[];
  outputs: { name: string; path: string; format: string }[];
}

export interface RegisteredApp {
  manifest: ManifestPreview;
  registeredAt: string;
  updatedAt: string;
}

export interface RegistryInfo {
  dataDir: string;
  databasePath: string;
  appVersion: string;
  os: string;
  arch: string;
}

export interface RecordConsumerDeployment {
  appId: string;
  stageId: string;
  consumerId: string;
  expectedManifestHash: string;
  expectedUpdatedAt: string;
  releaseRef: string;
  notes: string | null;
  approved: boolean;
}

export interface ConsumerDeployment {
  id: string;
  appId: string;
  appName: string;
  stageId: string;
  consumer: ConsumerSummary;
  releaseRef: string;
  notes: string | null;
  manifestHash: string;
  manifestPath: string;
  sourcePath: string;
  registrationUpdatedAt: string;
  recordedAt: string;
}

export interface PromotionContext {
  appId: string;
  stageId: string;
  expectedManifestHash: string;
  expectedUpdatedAt: string;
  releaseRef: string;
}

export interface RecordHarnessResult extends PromotionContext {
  checkId: string;
  total: number;
  passed: number;
  notes: string | null;
}

export interface HarnessResult {
  id: string;
  appId: string;
  appName: string;
  stageId: string;
  checkId: string;
  releaseRef: string;
  total: number;
  passed: number;
  notes: string | null;
  manifestHash: string;
  manifestPath: string;
  sourcePath: string;
  registrationUpdatedAt: string;
  recordedAt: string;
}

export interface CriterionEvaluation {
  criterion: PromotionCriterion;
  status: "satisfied" | "blocked" | "warning";
  message: string;
  evidenceId: string | null;
  observed: { total: number; passed: number } | null;
  consumer: ConsumerSummary | null;
}

export interface PromotionAssessment {
  appId: string;
  appName: string;
  stageId: string;
  releaseRef: string;
  manifestHash: string;
  manifestPath: string;
  sourcePath: string;
  registrationUpdatedAt: string;
  status: "not_configured" | "blocked" | "ready" | "ready_with_warnings";
  criteria: CriterionEvaluation[];
  stageRequirements: string[];
  assessmentHash: string;
}

export interface ApprovePromotion extends PromotionContext {
  expectedAssessmentHash: string;
  approved: boolean;
  acknowledgeOptional: boolean;
  notes: string | null;
}

export interface PromotionApproval {
  id: string;
  assessment: PromotionAssessment;
  skippedOptional: string[];
  notes: string | null;
  recordedAt: string;
}

export type RunStatus =
  | "pending"
  | "running"
  | "waiting_approval"
  | "passed"
  | "failed"
  | "cancelled"
  | "interrupted"
  | "blocked"
  | "skipped"
  | "timed_out";

export interface StartPipelineRun {
  appId: string;
  expectedManifestHash: string;
  expectedUpdatedAt: string;
  releaseRef: string;
  approved: boolean;
}

export interface StageRun {
  stageId: string;
  status: RunStatus;
  startedAt: string | null;
  finishedAt: string | null;
  approvedAt: string | null;
  message: string | null;
}

export interface CommandRun {
  stageId: string | null;
  commandId: string;
  status: RunStatus;
  startedAt: string | null;
  finishedAt: string | null;
  exitCode: number | null;
  message: string | null;
}

export interface PipelineRun {
  id: string;
  appId: string;
  appName: string;
  releaseRef: string;
  manifest: ManifestPreview;
  registrationUpdatedAt: string;
  status: RunStatus;
  startedAt: string;
  finishedAt: string | null;
  maxParallel: number;
  message: string | null;
  stages: StageRun[];
  commands: CommandRun[];
}

export interface PipelineRunSummary {
  id: string;
  appId: string;
  appName: string;
  releaseRef: string;
  manifestHash: string;
  registrationUpdatedAt: string;
  status: RunStatus;
  startedAt: string;
  finishedAt: string | null;
  message: string | null;
  stages: StageRun[];
  totalCommands: number;
  passedCommands: number;
}

export interface RunLogEntry {
  sequence: number;
  recordedAt: string;
  stageId: string | null;
  commandId: string | null;
  stream: "stdout" | "stderr" | "system";
  text: string;
}

export interface RunLogPage {
  entries: RunLogEntry[];
  nextCursor: number;
  hasMore: boolean;
}

export interface PipesApi {
  available: boolean;
  registryInfo(): Promise<RegistryInfo>;
  listApps(): Promise<RegisteredApp[]>;
  chooseManifest(): Promise<string | null>;
  previewManifest(path: string): Promise<ManifestPreview>;
  registerManifest(
    path: string,
    expectedHash: string,
    replace: boolean,
  ): Promise<RegisteredApp>;
  unregisterApp(appId: string): Promise<void>;
  listConsumerDeployments(appId?: string): Promise<ConsumerDeployment[]>;
  recordConsumerDeployment(request: RecordConsumerDeployment): Promise<ConsumerDeployment>;
  listHarnessResults(appId?: string): Promise<HarnessResult[]>;
  recordHarnessResult(request: RecordHarnessResult): Promise<HarnessResult>;
  assessPromotion(request: PromotionContext): Promise<PromotionAssessment>;
  approvePromotion(request: ApprovePromotion): Promise<PromotionApproval>;
  listPromotionApprovals(appId?: string): Promise<PromotionApproval[]>;
  listPipelineRuns(appId?: string): Promise<PipelineRunSummary[]>;
  startPipelineRun(request: StartPipelineRun): Promise<PipelineRun>;
  getPipelineRun(runId: string): Promise<PipelineRun>;
  pipelineRunLogs(runId: string, after: number): Promise<RunLogPage>;
  cancelPipelineRun(runId: string): Promise<PipelineRun>;
  approvePipelineStage(runId: string, stageId: string, approved: boolean): Promise<PipelineRun>;
}
