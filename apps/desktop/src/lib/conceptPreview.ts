import type { StageTelemetry, StageTone, TransitionTelemetry } from "../components/StageFlow";
import type { PromotionReview } from "./promotion";
import type { CommandSummary, ConsumerDeployment, ConsumerSummary, HarnessResult, PromotionCriterion, StageSummary } from "./types";

export type PreviewProjectId = "atlas" | "beacon" | "relay";
export type AtlasPreset = "executing" | "gate" | "promoted" | "live";
export const previewStageIds = ["test", "beta", "gamma", "prod"] as const;
export type PreviewStageId = typeof previewStageIds[number];

export interface SampleArtifact {
  name: string;
  hash: string;
  size: string;
  bytes: number;
  description: string;
}

export interface SampleProject {
  id: PreviewProjectId;
  version: string;
  previousVersion: string;
  target: string;
  commit: string;
  manifestHash: string;
  run: number;
  signature: string;
  artifacts: readonly SampleArtifact[];
  manifest: string;
  stages: StageSummary[];
}

export interface SampleCheck {
  name: string;
  description: string;
  duration: string;
  result: string;
  tone: StageTone;
}

export interface SampleEvent {
  time: string;
  stage: string;
  message: string;
  tone: StageTone;
}

export interface PreviewSnapshot {
  status: string;
  tone: StageTone;
  selectedStage: PreviewStageId;
  currentLive: string;
  stages: Record<PreviewStageId, StageTelemetry>;
  transitions: TransitionTelemetry[];
  checks: SampleCheck[];
  events: SampleEvent[];
}

export const previewStages: StageSummary[] = previewStageIds.map((id, index) => ({
  id,
  after: index === 0 ? [] : [previewStageIds[index - 1]!],
  checks: [],
  checkPlan: [],
  requires: index === 0 ? [] : id === "gamma"
    ? ["previous.passed", "checks.passed", "approval.local"]
    : ["previous.passed", "checks.passed"],
  requiresApproval: id === "gamma",
  consumers: [],
  promotionCriteria: [],
}));

const sampleConsumers: Record<PreviewProjectId, ConsumerSummary> = {
  atlas: { id: "personal-mac", name: "Personal MacBook", target: "My MacBook / Applications/Atlas.app", approval: "manual" },
  beacon: { id: "home-server", name: "Home server", target: "Home server / beacon worker", approval: "manual" },
  relay: { id: "dev-laptop", name: "Dev laptop", target: "Personal laptop / ~/.local/bin/relay", approval: "manual" },
};

const atlasGammaConsumers: ConsumerSummary[] = [
  { id: "lab-device", name: "Lab Workstation", target: "Sample lab workstation / Applications/Atlas.app", approval: "manual" },
  { id: "personal-canary", name: "Personal Canary Device", target: "Sample personal device / Applications/Atlas.app", approval: "manual" },
];

const atlasGammaCriteria: PromotionCriterion[] = [
  { id: "canary-tests", kind: "harness", checkId: "canary", expectedTests: 50, minimumPassed: 45 },
  { id: "lab-delivery", kind: "deployment", consumerId: "lab-device", requirement: "required" },
  { id: "personal-delivery", kind: "deployment", consumerId: "personal-canary", requirement: "optional" },
];

const atlasCanary: CommandSummary = {
  id: "canary", exec: ["./ci/canary.sh"], cwd: null, timeout: null, env: {}, secrets: {},
};

function projectStages(id: PreviewProjectId): StageSummary[] {
  return previewStages.map((stage) => {
    if (stage.id === "beta") return { ...stage, consumers: [sampleConsumers[id]] };
    if (id === "atlas" && stage.id === "gamma") return {
      ...stage,
      checks: [atlasCanary],
      checkPlan: [{ kind: "command", command: atlasCanary }],
      consumers: atlasGammaConsumers,
      promotionCriteria: atlasGammaCriteria,
    };
    return stage;
  });
}

function sampleManifest(id: PreviewProjectId, target: string, test: string, artifact: string) {
  const consumer = sampleConsumers[id];
  const gammaPolicy = id === "atlas" ? `
    consumers:
${atlasGammaConsumers.map((item) => `      - id: ${item.id}
        name: ${JSON.stringify(item.name)}
        target: ${JSON.stringify(item.target)}
        approval: manual`).join("\n")}
    promotion:
      criteria:
        - id: canary-tests
          kind: harness
          check: canary
          expected_tests: 50
          minimum_passed: 45
        - id: lab-delivery
          kind: deployment
          consumer: lab-device
          requirement: required
        - id: personal-delivery
          kind: deployment
          consumer: personal-canary
          requirement: optional` : "";
  return `# DESIGN PREVIEW — illustrative text only; never executed or registered.
api: pipes/v1
app: ${id}
source:
  path: .
  snapshot: git-commit
  require_clean: true
version:
  scheme: semver
  from: VERSION
execution:
  host: ${target}
  timeout: 20m
outputs:
  release:
    path: ${artifact}
    format: file
stages:
  - id: test
    checks:
      - id: tests
        exec: ${test}
  - id: beta
    after: [test]
    checks:
      - id: smoke
        exec: [./ci/smoke.sh]
    require: [previous.passed, checks.passed]
    consumers:
      - id: ${consumer.id}
        name: ${JSON.stringify(consumer.name)}
        target: ${JSON.stringify(consumer.target)}
        approval: manual
  - id: gamma
    after: [beta]
    checks:
      - id: canary
        exec: [./ci/canary.sh]
    require: [previous.passed, checks.passed, approval.local]${gammaPolicy}
  - id: prod
    after: [gamma]
    checks:
      - id: health
        exec: [./ci/health.sh]
    require: [previous.passed, checks.passed]`;
}

export const sampleProjects: readonly SampleProject[] = [
  {
    id: "atlas",
    version: "1.5.0",
    previousVersion: "1.4.2",
    target: "macOS / arm64",
    commit: "8f1c2a9",
    manifestHash: "6e4a…b821",
    run: 183,
    signature: "Developer ID (sample)",
    artifacts: [
      { name: "Atlas-1.5.0-macos-arm64.zip", hash: "88ab…e43c", size: "28.4 MB", bytes: 28_400_000, description: "sample signed bundle" },
      { name: "test-results.xcresult.zip", hash: "04ec…ab21", size: "6.2 MB", bytes: 6_200_000, description: "sample test report" },
    ],
    manifest: sampleManifest("atlas", "{ os: macos, arch: arm64 }", "[./ci/test.sh]", "dist/Atlas.zip"),
    stages: projectStages("atlas"),
  },
  {
    id: "beacon",
    version: "2.8.1",
    previousVersion: "2.7.4",
    target: "python · worker",
    commit: "b70d3e1",
    manifestHash: "91de…3b08",
    run: 76,
    signature: "not signed · test failed",
    artifacts: [
      { name: "beacon-pytest-report.xml", hash: "bf10…29ad", size: "80 KB", bytes: 80_000, description: "sample failure report; no release bundle" },
    ],
    manifest: sampleManifest("beacon", "{ os: linux, arch: x86_64 }", "[pytest, tests]", "dist/beacon.whl"),
    stages: projectStages("beacon"),
  },
  {
    id: "relay",
    version: "0.9.3",
    previousVersion: "0.9.2",
    target: "rust · cli",
    commit: "c42e90a",
    manifestHash: "54bc…ca17",
    run: 42,
    signature: "release signature (sample)",
    artifacts: [
      { name: "relay-0.9.3.tar.gz", hash: "7e19…ca32", size: "7.1 MB", bytes: 7_100_000, description: "sample release binary" },
      { name: "relay-0.9.3.sha256", hash: "b9fe…21d0", size: "1 KB", bytes: 1_000, description: "sample checksum text" },
    ],
    manifest: sampleManifest("relay", "{ os: macos, arch: arm64 }", "[cargo, test]", "target/release/relay"),
    stages: projectStages("relay"),
  },
];

export const sampleStorage = {
  releases: sampleProjects.length,
  artifacts: sampleProjects.reduce((count, project) => count + project.artifacts.length, 0),
  megabytes: (sampleProjects.reduce(
    (bytes, project) => bytes + project.artifacts.reduce((sum, artifact) => sum + artifact.bytes, 0),
    0,
  ) / 1_000_000).toFixed(1),
};

export function getSampleProject(id: PreviewProjectId): SampleProject {
  return sampleProjects.find((project) => project.id === id)!;
}

export const sampleRegistrationUpdatedAt = "2026-10-26T10:00:00Z";
export const sampleRecordEpoch = "2026-10-26T12:30:00Z";

export function getSamplePromotionReview(id: PreviewProjectId, stageId: PreviewStageId): PromotionReview {
  const project = getSampleProject(id);
  const stage = project.stages.find((item) => item.id === stageId);
  if (!stage) throw { code: "stage_not_found", message: `Sample stage ${stageId} is not configured for ${id}.` };
  return {
    appId: project.id,
    appName: project.id,
    stage: structuredClone(stage),
    manifestHash: project.manifestHash,
    manifestPath: `/preview/${project.id}/pipes.yaml`,
    sourcePath: `/preview/${project.id}`,
    updatedAt: sampleRegistrationUpdatedAt,
  };
}

export const sampleConsumerDeployments: readonly ConsumerDeployment[] = [{
  id: "preview-1",
  appId: "atlas",
  appName: "atlas",
  stageId: "beta",
  consumer: { ...sampleConsumers.atlas },
  releaseRef: "v1.4.2",
  notes: "Sample: copied the previous beta to my personal laptop.",
  manifestHash: getSampleProject("atlas").manifestHash,
  manifestPath: "/preview/atlas/pipes.yaml",
  sourcePath: "/preview/atlas",
  registrationUpdatedAt: sampleRegistrationUpdatedAt,
  recordedAt: "2026-10-26T12:06:00Z",
}, {
  id: "preview-fb33e7b6-cf1c-4451-bd94-9f87029c0421",
  appId: "atlas",
  appName: "atlas",
  stageId: "gamma",
  consumer: { ...atlasGammaConsumers[0]! },
  releaseRef: "v1.5.0",
  notes: "Sample: candidate manually delivered to the lab workstation.",
  manifestHash: getSampleProject("atlas").manifestHash,
  manifestPath: "/preview/atlas/pipes.yaml",
  sourcePath: "/preview/atlas",
  registrationUpdatedAt: sampleRegistrationUpdatedAt,
  recordedAt: "2026-10-26T12:19:00Z",
}];

export const sampleHarnessResults: readonly HarnessResult[] = [{
  id: "preview-a5983d50-97a3-4f3b-8d99-0c328ffdd440",
  appId: "atlas",
  appName: "atlas",
  stageId: "gamma",
  checkId: "canary",
  releaseRef: "v1.5.0",
  total: 50,
  passed: 45,
  notes: "Sample externally reported counts: 45 passed and 5 failed or skipped; no harness was executed.",
  manifestHash: getSampleProject("atlas").manifestHash,
  manifestPath: "/preview/atlas/pipes.yaml",
  sourcePath: "/preview/atlas",
  registrationUpdatedAt: sampleRegistrationUpdatedAt,
  recordedAt: "2026-10-26T12:18:00Z",
}];

const roles: Record<PreviewStageId, string> = {
  test: "VERIFY", beta: "SOAK", gamma: "CANARY", prod: "RELEASE",
};

function telemetry(id: PreviewStageId, data: Omit<StageTelemetry, "role">): StageTelemetry {
  return { role: roles[id], ...data };
}

const atlasTest = telemetry("test", {
  tone: "done", status: "PASSED", duration: "2m 14s", summary: "20/20 checks",
  progress: 100, detail: "xcresult verified", metric: "100%",
});
const atlasBeta = telemetry("beta", {
  tone: "done", status: "PASSED", duration: "1m 08s", summary: "4/4 smoke pass",
  progress: 100, detail: "signed bundle", metric: "100%",
});
const atlasLocked = telemetry("prod", {
  tone: "idle", status: "LOCKED", duration: "retained", summary: "live v1.4.2",
  progress: 0, detail: "awaiting gamma", metric: "gate",
});
const atlasApproved = telemetry("gamma", {
  tone: "done", status: "APPROVED", duration: "1m 19s", summary: "canary 4/4 passed",
  progress: 100, detail: "visual scenario only", metric: "100%",
});

const atlasGamma: Record<AtlasPreset, StageTelemetry> = {
  executing: telemetry("gamma", {
    tone: "running", status: "RUNNING", duration: "0m 48s", summary: "canary 3/4 checks",
    progress: 74, detail: "step 3: telemetry", metric: "74%",
  }),
  gate: telemetry("gamma", {
    tone: "waiting", status: "GATE", duration: "ready", summary: "awaiting approval",
    progress: 100, detail: "canary 4/4 passed", metric: "approve →",
  }),
  promoted: atlasApproved,
  live: atlasApproved,
};

const atlasProd: Record<AtlasPreset, StageTelemetry> = {
  executing: atlasLocked,
  gate: atlasLocked,
  promoted: telemetry("prod", {
    tone: "running", status: "DEPLOYING", duration: "0m 19s", summary: "promoting v1.5.0",
    progress: 62, detail: "swapping symlink", metric: "62%",
  }),
  live: telemetry("prod", {
    tone: "live", status: "LIVE", duration: "0m 31s", summary: "v1.5.0 active",
    progress: 100, detail: "promotion complete", metric: "healthy",
  }),
};

const atlasStatus: Record<AtlasPreset, { status: string; tone: StageTone }> = {
  executing: { status: "promoting → gamma", tone: "running" },
  gate: { status: "approval gate", tone: "waiting" },
  promoted: { status: "promoting → prod", tone: "running" },
  live: { status: "v1.5.0 live", tone: "live" },
};

function atlasSnapshot(preset: AtlasPreset): PreviewSnapshot {
  const canaryComplete = preset !== "executing";
  const approved = preset === "promoted" || preset === "live";
  const checks: SampleCheck[] = [
    { name: "build", description: "xcodebuild Release arm64", duration: "1m 12s", result: "pass", tone: "done" },
    { name: "unit + UI", description: "20 test suites", duration: "2m 14s", result: "20/20", tone: "done" },
    { name: "signature", description: "Developer ID strict verify", duration: "0m 04s", result: "valid", tone: "done" },
    {
      name: "canary", description: canaryComplete ? "4/4 checks complete" : "3/4 checks · telemetry executing",
      duration: canaryComplete ? "1m 19s" : "0m 48s",
      result: canaryComplete ? "4/4" : "running 74%", tone: canaryComplete ? "done" : "running",
    },
  ];
  if (approved) {
    checks.push({
      name: "prod readiness",
      description: preset === "live" ? "release healthy · v1.5.0 active" : "deploy + health verification",
      duration: preset === "live" ? "0m 31s" : "0m 19s",
      result: preset === "live" ? "healthy" : "running 62%",
      tone: preset === "live" ? "done" : "running",
    });
  }
  const events: SampleEvent[] = [
    { time: "12:02:00", stage: "source", message: "atlas v1.5.0 · commit 8f1c2a9 captured", tone: "done" },
    { time: "12:12:08", stage: "bundle", message: "sample signed bundle prepared", tone: "done" },
    { time: "12:14:22", stage: "01/test", message: "unit + UI · 20/20 passed (2m 14s)", tone: "done" },
    { time: "12:16:09", stage: "02/beta", message: "smoke suite · 4/4 passed (1m 08s)", tone: "done" },
  ];
  if (!canaryComplete) {
    events.push({ time: "12:18:41", stage: "03/gamma", message: "beta → gamma · canary 3/4 checks · executing at 74%", tone: "running" });
  } else {
    events.push({ time: "12:19:12", stage: "03/gamma", message: "canary 4/4 passed · awaiting approval", tone: "waiting" });
    if (approved) {
      events.push({ time: "12:20:00", stage: "scenario", message: "visual scenario snapshot · promoting → prod; recorded approvals are listed separately", tone: "done" });
      events.push(preset === "live"
        ? { time: "12:20:31", stage: "04/prod", message: "v1.5.0 active · promotion complete · healthy", tone: "live" }
        : { time: "12:20:19", stage: "04/prod", message: "deploy + health verification · executing at 62%", tone: "running" });
    }
  }
  return {
    ...atlasStatus[preset],
    selectedStage: approved ? "prod" : "gamma",
    currentLive: preset === "live" ? "1.5.0" : "1.4.2",
    stages: { test: atlasTest, beta: atlasBeta, gamma: atlasGamma[preset], prod: atlasProd[preset] },
    transitions: [
      { from: "test", to: "beta", tone: "done", label: "promoted" },
      { from: "beta", to: "gamma", tone: preset === "executing" ? "promoting" : "done", label: preset === "executing" ? "promoting" : "promoted" },
      {
        from: "gamma", to: "prod",
        tone: preset === "executing" ? "idle" : preset === "gate" ? "next" : preset === "promoted" ? "promoting" : "done",
        label: preset === "executing" ? "locked" : preset === "gate" ? "approve" : preset === "promoted" ? "promoting" : "live",
      },
    ],
    checks,
    events,
  };
}

const beaconSnapshot: PreviewSnapshot = {
  status: "stopped at test",
  tone: "failed",
  selectedStage: "test",
  currentLive: "2.7.4",
  stages: {
    test: telemetry("test", {
      tone: "failed", status: "FAILED", duration: "0m 52s", summary: "2/12 failed",
      progress: 82, detail: "exit code 1", metric: "10/12",
    }),
    beta: telemetry("beta", {
      tone: "idle", status: "BLOCKED", duration: "—", summary: "blocked by test",
      progress: 0, detail: "upstream failed", metric: "locked",
    }),
    gamma: telemetry("gamma", {
      tone: "idle", status: "BLOCKED", duration: "—", summary: "blocked by test",
      progress: 0, detail: "upstream failed", metric: "locked",
    }),
    prod: telemetry("prod", {
      tone: "idle", status: "RETAINED", duration: "3d ago", summary: "live v2.7.4",
      progress: 0, detail: "prod untouched", metric: "v2.7.4",
    }),
  },
  transitions: [
    { from: "test", to: "beta", tone: "idle", label: "halted" },
    { from: "beta", to: "gamma", tone: "idle", label: "blocked" },
    { from: "gamma", to: "prod", tone: "idle", label: "retained" },
  ],
  checks: [
    { name: "worker tests", description: "10 assertions", duration: "0m 46s", result: "10/10", tone: "done" },
    { name: "queue contract", description: "2 assertions failed", duration: "0m 06s", result: "2 failed", tone: "failed" },
  ],
  events: [
    { time: "12:14:00", stage: "source", message: "beacon v2.8.1 · commit b70d3e1 captured", tone: "done" },
    { time: "12:15:00", stage: "01/test", message: "10/12 passed · 2 assertions failed · exit code 1", tone: "failed" },
    { time: "12:15:01", stage: "pipeline", message: "promotion halted · prod v2.7.4 retained", tone: "failed" },
  ],
};

const relaySnapshot: PreviewSnapshot = {
  status: "v0.9.3 live",
  tone: "live",
  selectedStage: "prod",
  currentLive: "0.9.3",
  stages: {
    test: telemetry("test", {
      tone: "done", status: "PASSED", duration: "1m 05s", summary: "48/48 passed",
      progress: 100, detail: "cargo test", metric: "100%",
    }),
    beta: telemetry("beta", {
      tone: "done", status: "PASSED", duration: "0m 38s", summary: "smoke verified",
      progress: 100, detail: "binary signed", metric: "100%",
    }),
    gamma: telemetry("gamma", {
      tone: "done", status: "PASSED", duration: "4m 10s", summary: "soak 100%",
      progress: 100, detail: "0 regressions", metric: "100%",
    }),
    prod: telemetry("prod", {
      tone: "live", status: "LIVE", duration: "11:42", summary: "v0.9.3 active",
      progress: 100, detail: "promoted clean", metric: "healthy",
    }),
  },
  transitions: [
    { from: "test", to: "beta", tone: "done", label: "promoted" },
    { from: "beta", to: "gamma", tone: "done", label: "promoted" },
    { from: "gamma", to: "prod", tone: "done", label: "live" },
  ],
  checks: [
    { name: "unit tests", description: "cargo test · 48 assertions", duration: "1m 05s", result: "48/48", tone: "done" },
    { name: "signature", description: "release binary", duration: "0m 02s", result: "valid", tone: "done" },
    { name: "smoke", description: "CLI acceptance", duration: "0m 38s", result: "pass", tone: "done" },
    { name: "soak", description: "0 regressions", duration: "4m 10s", result: "100%", tone: "done" },
  ],
  events: [
    { time: "11:35:00", stage: "source", message: "relay v0.9.3 · commit c42e90a captured", tone: "done" },
    { time: "11:36:05", stage: "01/test", message: "cargo test · 48/48 passed", tone: "done" },
    { time: "11:36:43", stage: "02/beta", message: "smoke verified · sample binary signed", tone: "done" },
    { time: "11:40:53", stage: "03/gamma", message: "soak complete · 0 regressions", tone: "done" },
    { time: "11:42:00", stage: "04/prod", message: "v0.9.3 active · healthy", tone: "live" },
  ],
};

export function getPreviewSnapshot(id: PreviewProjectId, preset: AtlasPreset): PreviewSnapshot {
  if (id === "atlas") return atlasSnapshot(preset);
  return id === "beacon" ? beaconSnapshot : relaySnapshot;
}

export const atlasPresets: readonly { id: Exclude<AtlasPreset, "live">; label: string }[] = [
  { id: "executing", label: "01 executing + promoting" },
  { id: "gate", label: "02 approval gate" },
  { id: "promoted", label: "03 prod promoting" },
];

export const atlasNextStep: Record<AtlasPreset, { next: AtlasPreset; label: string; notice: string }> = {
  executing: {
    next: "gate", label: "complete checks",
    notice: "Preview only — Atlas canary checks completed. The approval gate is waiting; prod remains v1.4.2.",
  },
  gate: {
    next: "promoted", label: "approve & promote",
    notice: "Preview only — Atlas approval simulated. Prod promotion is at 62%; v1.4.2 is still retained.",
  },
  promoted: {
    next: "live", label: "finish deployment",
    notice: "Preview only — Atlas v1.5.0 is now live in the sample. No deployment occurred.",
  },
  live: {
    next: "executing", label: "reset simulation",
    notice: "Preview only — Atlas reset to the executing snapshot. No pipeline or command started.",
  },
};
