import { useId, useMemo, useState } from "react";
import { usePipelineRun } from "../hooks/usePipelineRun";
import { isRunActive, matchesRunRegistration, runDuration, runStageTelemetry, stageWaitKind, summarizeRun } from "../lib/pipelineRuns";
import type { PipelineRun, PipesApi, RegisteredApp, StageSummary } from "../lib/types";
import { CommandList } from "./CommandList";
import { ErrorBanner } from "./ErrorBanner";
import { Arrow } from "./Icons";
import { RunStatusBadge } from "./PipelineRunHistory";
import { RunTerminal } from "./RunTerminal";
import { StageFlow } from "./StageFlow";
import "./PipelineRuns.css";

function StageApproval({
  run,
  stage,
  busy,
  disabled,
  onApprove,
}: {
  run: PipelineRun;
  stage: StageSummary;
  busy: boolean;
  disabled: boolean;
  onApprove(): void;
}) {
  const id = useId();
  const [approved, setApproved] = useState(false);
  return (
    <form className="run-stage-approval" onSubmit={(event) => {
      event.preventDefault();
      if (approved && !busy && !disabled) {
        setApproved(false);
        onApprove();
      }
    }}>
      <h3>approval.local / {stage.id}</h3>
      <p>This stage is waiting BEFORE its commands. This approval applies only to <strong>{run.releaseRef}</strong> in run <code>{run.id}</code>, not future runs or custom promotion policies.</p>
      <label className="execution-approval" htmlFor={`${id}-approve`}>
        <input id={`${id}-approve`} type="checkbox" checked={approved} disabled={busy || disabled} onChange={(event) => setApproved(event.currentTarget.checked)} />
        <span>I approve executing stage {stage.id} for this run.</span>
      </label>
      <button type="submit" className="button primary full" disabled={!approved || busy || disabled}>approve stage {stage.id}</button>
    </form>
  );
}

export function PipelineRunDetail({
  api,
  runId,
  apps,
  registrationsLoaded = true,
  onUpdate,
  onBack,
  onShowConfiguration,
}: {
  api: PipesApi;
  runId: string;
  apps: RegisteredApp[];
  registrationsLoaded?: boolean;
  onUpdate(run: PipelineRun): void;
  onBack(): void;
  onShowConfiguration(appId: string, stageId?: string, promotion?: boolean, releaseRef?: string): void;
}) {
  const state = usePipelineRun(api, runId, onUpdate);
  const [selectedStage, setSelectedStage] = useState("");
  const [stageFilter, setStageFilter] = useState("");
  const run = state.run;
  const telemetry = useMemo(() => run ? runStageTelemetry(run) : undefined, [run]);
  const registration = apps.find((app) => app.manifest.appId === run?.appId);
  const matches = run ? matchesRunRegistration(summarizeRun(run), registration) : false;
  const stage = run?.manifest.stages.find((item) => item.id === selectedStage) ??
    run?.manifest.stages.find((item) => run.stages.some((state) => state.stageId === item.id && (state.status === "waiting_approval" || state.status === "running"))) ??
    run?.manifest.stages[0];
  const stageRun = run?.stages.find((item) => item.stageId === stage?.id);
  const waiting = stage && run ? stageWaitKind(stage, stageRun, run.commands.filter((command) => command.stageId === stage.id)) : null;
  const stageMessage = stage?.requiresApproval && stageRun?.status === "waiting_approval" &&
    stageRun.approvedAt !== null && stageRun.startedAt === null ? null : stageRun?.message;

  return (
    <section className="pipeline-run-detail" aria-label="Pipeline run details">
      <div className="detail-toolbar">
        <button type="button" className="text-button back-link" onClick={onBack}><Arrow />run history</button>
        <button type="button" className="text-button" onClick={state.refresh}>refresh run</button>
      </div>
      {state.error && <ErrorBanner error={state.error} />}
      {state.error && run && <p className="compact-note">Showing the last received run state and output. Retrying uses the saved log cursor.</p>}
      {state.actionError && <ErrorBanner error={state.actionError} />}
      {state.loading && <p className="loading" role="status">Loading run and persisted output...</p>}
      {run && (
        <>
          <header className="run-heading">
            <div>
              <span className="eyebrow">recorded execution / {run.appId}</span>
              <h2>{run.appName} / {run.releaseRef}</h2>
              <code className="run-id">{run.id}</code>
            </div>
            <div className="actions">
              <span role="status" aria-label="Run status"><RunStatusBadge status={run.status} /></span>
              {isRunActive(run.status) && <button type="button" className="button danger" disabled={state.actionBusy || state.cancellationRequested} onClick={state.cancel}>
                {state.cancellationRequested ? "cancellation requested" : "cancel run"}
              </button>}
            </div>
          </header>
          {run.message && <p className={`run-message ${run.status === "failed" || run.status === "blocked" || run.status === "timed_out" || run.status === "interrupted" ? "run-message-failure" : ""}`} role="status">{run.message}</p>}
          {state.cancellationRequested && isRunActive(run.status) && <p className="compact-note" role="status">Cancellation requested. Waiting for running commands to stop; remaining output will still be collected.</p>}
          <div className="run-snapshot-context">
            <strong>Frozen run manifest</strong>
            <span>{!registrationsLoaded ? "Registration status unavailable. The frozen run remains inspectable." : !registration ? "App unregistered. History and output are retained." : matches ? "Matches the last loaded registration; source files may have changed." : "Different from the current registration. Historic results are not applied to its new configuration."}</span>
            {registration && <button type="button" className="text-button" onClick={() => onShowConfiguration(run.appId)}>view current configuration <Arrow /></button>}
          </div>
          <dl className="run-stats">
            <div><dt>elapsed</dt><dd>{runDuration(run.startedAt, run.finishedAt)}</dd></div>
            <div><dt>stages passed</dt><dd>{run.stages.filter((stage) => stage.status === "passed").length}/{run.stages.length}</dd></div>
            <div><dt>commands passed</dt><dd>{run.commands.filter((command) => command.status === "passed").length}/{run.commands.length}</dd></div>
            <div><dt>parallel budget</dt><dd>{run.maxParallel} {run.maxParallel === 1 ? "command" : "commands"}</dd></div>
          </dl>
          <section className="state-machine" aria-label="Recorded stage progress">
            <div className="machine-bar"><h3 className="eyebrow">Run stages / real outcomes</h3><span className="selected-hint">select a stage to inspect and filter output</span></div>
            <StageFlow
              stages={run.manifest.stages}
              telemetry={telemetry}
              label={`${run.appName} run stages`}
              selectedStage={stage?.id}
              onSelectStage={(id) => { setSelectedStage(id); setStageFilter(id); }}
              variant="detail"
            />
          </section>
          <div className="run-inspection-grid">
            <div className="run-main">
              <RunTerminal run={run} output={state.output} cursor={state.cursor} stageFilter={stageFilter} onStageFilter={setStageFilter} />
              <details className="card run-command-records" open>
                <summary>command outcomes / {run.commands.length} {run.commands.length === 1 ? "command" : "commands"}</summary>
                <p className="panel-caption">Command outcomes, not test counts. Expand a command to inspect its frozen argv, cwd, environment, and result contract.</p>
                {run.commands.length === 0 ? <p className="compact-note">No commands were configured for this run.</p> : (
                  <div className="run-command-scroll" tabIndex={0} role="region" aria-label="Recorded command outcomes">
                    <table className="run-command-table">
                      <thead><tr><th scope="col">stage / command</th><th scope="col">outcome</th><th scope="col">duration / exit</th></tr></thead>
                      <tbody>{run.commands.map((command) => {
                        const definition = command.stageId === null ? run.manifest.prepare.find((item) => item.id === command.commandId) :
                          run.manifest.stages.find((item) => item.id === command.stageId)?.checks.find((item) => item.id === command.commandId);
                        return (
                          <tr key={JSON.stringify([command.stageId, command.commandId])}>
                            <th scope="row">
                              <details>
                                <summary>{command.stageId ?? "prepare"} / {command.commandId}</summary>
                                {definition && <CommandList commands={[definition]} sourcePath={run.manifest.sourcePath} />}
                                <dl className="command-timestamps">
                                  <dt>started</dt><dd>{command.startedAt ?? "not started"}</dd>
                                  <dt>finished</dt><dd>{command.finishedAt ?? "not finished"}</dd>
                                </dl>
                              </details>
                            </th>
                            <td><RunStatusBadge status={command.status} />{command.message && <p className="command-message">{command.message}</p>}</td>
                            <td>{runDuration(command.startedAt, command.finishedAt)}<br />exit: {command.exitCode ?? "—"}</td>
                          </tr>
                        );
                      })}</tbody>
                    </table>
                  </div>
                )}
              </details>
            </div>
            <aside className="run-inspector" aria-label="Run inspector">
              {stage && stageRun && (
                <section className="card run-stage-inspector" aria-label={`${stage.id} run status`}>
                  <header className="panel-heading"><h3>{stage.id}</h3><RunStatusBadge status={stageRun.status} /></header>
                  {stageMessage && <p className="run-stage-message">{stageMessage}</p>}
                  <dl className="metadata">
                    <div><dt>started</dt><dd>{stageRun.startedAt ?? "not started"}</dd></div>
                    <div><dt>finished</dt><dd>{stageRun.finishedAt ?? "not finished"}</dd></div>
                    <div><dt>elapsed</dt><dd>{runDuration(stageRun.startedAt, stageRun.finishedAt)}</dd></div>
                    <div><dt>local approval</dt><dd>{stageRun.approvedAt ?? (stage.requiresApproval ? "not approved" : "not required")}</dd></div>
                  </dl>
                  {waiting === "local" && isRunActive(run.status) && <StageApproval key={`${run.id}:${stage.id}`} run={run} stage={stage} busy={state.actionBusy} disabled={state.cancellationRequested} onApprove={() => state.approve(stage.id)} />}
                  {waiting === "runner" && (
                    <div className="run-stage-wait">
                      <h4>{stageRun.approvedAt ? "Local approval recorded" : "Waiting for runner"}</h4>
                      <p>Waiting for the runner to update this stage. Shown command statuses are the latest received snapshot; no additional local approval is needed.</p>
                    </div>
                  )}
                  {waiting === "promotion" && (
                    <div className="run-policy-wait">
                      <h4>Waiting for custom promotion policy</h4>
                      <p>Command execution does not report harness counts or approve promotion criteria. Report evidence and explicitly approve the policy assessment for this release reference. Local execution approval cannot bypass it.</p>
                      {matches ? <button type="button" className="button full" onClick={() => onShowConfiguration(run.appId, stage.id, true, run.releaseRef)}>review promotion criteria <Arrow /></button> :
                        <p>The current registration no longer matches this run. The policy gate remains closed.</p>}
                    </div>
                  )}
                </section>
              )}
              <section className="card run-source-inspector">
                <header className="panel-heading"><h3 className="eyebrow">execution context</h3></header>
                <dl className="metadata">
                  <div><dt>source / default cwd</dt><dd>{run.manifest.sourcePath}</dd></div>
                  <div><dt>mode</dt><dd>current working tree</dd></div>
                  <div><dt>started</dt><dd>{run.startedAt}</dd></div>
                  <div><dt>finished</dt><dd>{run.finishedAt ?? "not finished"}</dd></div>
                  <div><dt>manifest hash</dt><dd>{run.manifest.manifestHash}</dd></div>
                  <div><dt>registration revision</dt><dd>{run.registrationUpdatedAt}</dd></div>
                </dl>
                <p className="compact-note">Attached to the app/CLI that started it. Shutdown interrupts active runs. Declared outputs are not automatically collected, signed, or verified releases.</p>
              </section>
            </aside>
          </div>
          <details className="disclosure run-frozen-manifest">
            <summary>frozen run manifest / {run.manifest.manifestPath}</summary>
            <pre className="yaml">{run.manifest.manifestYaml}</pre>
          </details>
        </>
      )}
    </section>
  );
}
