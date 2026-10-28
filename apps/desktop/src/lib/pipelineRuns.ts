import type { StageFlowTelemetry, StageTone } from "../components/StageFlow";
import type { CommandRun, PipelineRun, PipelineRunSummary, RegisteredApp, RunStatus, StageRun, StageSummary } from "./types";

export const PIPELINE_RUN_HISTORY_LIMIT = 100;

export function isRunActive(status: RunStatus): boolean {
  return status === "pending" || status === "running" || status === "waiting_approval";
}

export function runStatusLabel(status: RunStatus): string {
  return status.replaceAll("_", " ");
}

export function runTone(status: RunStatus): StageTone {
  switch (status) {
    case "passed": return "done";
    case "running": return "running";
    case "waiting_approval": return "waiting";
    case "failed":
    case "timed_out":
    case "interrupted":
    case "blocked": return "failed";
    default: return "idle";
  }
}

export function runDuration(start: string | null, finish: string | null, now = Date.now()): string {
  if (!start) return "--:--";
  const elapsed = (finish ? Date.parse(finish) : now) - Date.parse(start);
  if (!Number.isFinite(elapsed)) return "--:--";
  const seconds = Math.max(0, Math.floor(elapsed / 1000));
  const hours = Math.floor(seconds / 3600);
  return `${hours > 0 ? `${hours}:` : ""}${String(Math.floor(seconds / 60) % 60).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
}

export function summarizeRun(run: PipelineRun): PipelineRunSummary {
  return {
    id: run.id,
    appId: run.appId,
    appName: run.appName,
    releaseRef: run.releaseRef,
    manifestHash: run.manifest.manifestHash,
    registrationUpdatedAt: run.registrationUpdatedAt,
    status: run.status,
    startedAt: run.startedAt,
    finishedAt: run.finishedAt,
    message: run.message,
    stages: run.stages,
    totalCommands: run.commands.length,
    passedCommands: run.commands.filter((command) => command.status === "passed").length,
  };
}

export function matchesRunRegistration(run: PipelineRunSummary, app?: RegisteredApp): boolean {
  return Boolean(app && app.manifest.appId === run.appId &&
    app.manifest.manifestHash === run.manifestHash && app.updatedAt === run.registrationUpdatedAt);
}

export function stageWaitKind(stage: StageSummary, state: StageRun | undefined, commands: readonly CommandRun[]): "local" | "promotion" | "runner" | null {
  if (state?.status !== "waiting_approval") return null;
  if (stage.requiresApproval && state.approvedAt === null) return "local";
  if (stage.promotionCriteria.length > 0 && commands.length === stage.checks.length &&
    commands.every((command) => command.status === "passed")) return "promotion";
  return "runner";
}

export function runStageTelemetry(run: PipelineRun): StageFlowTelemetry {
  return {
    stages: Object.fromEntries(run.manifest.stages.map((stage) => {
      const state = run.stages.find((item) => item.stageId === stage.id);
      const commands = run.commands.filter((command) => command.stageId === stage.id);
      const passed = commands.filter((command) => command.status === "passed").length;
      const completed = commands.filter((command) => !isRunActive(command.status)).length;
      const status = state?.status ?? "pending";
      const waiting = stageWaitKind(stage, state, commands);
      return [stage.id, {
        tone: runTone(status),
        role: stage.after.length === 0 ? "entry" : stage.after.length > 1 ? "join / all" : "stage",
        status: runStatusLabel(status),
        duration: runDuration(state?.startedAt ?? null, state?.finishedAt ?? null),
        summary: `${passed}/${commands.length} commands passed`,
        progress: commands.length ? completed / commands.length * 100 : status === "passed" ? 100 : 0,
        detail: waiting === "local" ? "approve before commands" : waiting === "promotion" ? "promotion policy gate" : waiting === "runner" ? "awaiting runner update" : state?.approvedAt ? "locally approved" : "recorded execution",
        metric: status === "passed" ? "stage passed" : `${completed} finished`,
      }];
    })),
    transitions: run.manifest.stages.flatMap((stage) => stage.after.map((from) => {
      const parent = run.stages.find((item) => item.stageId === from);
      return { from, to: stage.id, tone: parent?.status === "passed" ? "done" as const : "idle" as const, label: parent?.status === "passed" ? "passed" : "all parents must pass" };
    })),
  };
}
