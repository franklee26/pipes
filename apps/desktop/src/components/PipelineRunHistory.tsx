import { useId, useState } from "react";
import type { DisplayError } from "../lib/api";
import { matchesRunRegistration, PIPELINE_RUN_HISTORY_LIMIT, runDuration, runStatusLabel, runTone } from "../lib/pipelineRuns";
import type { PipelineRunSummary, RegisteredApp, RunStatus } from "../lib/types";
import { ErrorBanner } from "./ErrorBanner";
import { Arrow, StatusIcon } from "./Icons";
import "./PipelineRuns.css";

export function RunStatusBadge({ status }: { status: RunStatus }) {
  const tone = runTone(status);
  return <span className={`status-tag ${tone}`}><StatusIcon tone={tone} />{runStatusLabel(status)}</span>;
}

export function PipelineRunHistory({
  runs,
  loading,
  error,
  apps,
  registrationsLoaded = true,
  onSelect,
  onRefresh,
}: {
  runs: PipelineRunSummary[] | null;
  loading: boolean;
  error: DisplayError | null;
  apps: RegisteredApp[];
  registrationsLoaded?: boolean;
  onSelect(runId: string): void;
  onRefresh(): void;
}) {
  const id = useId();
  const [query, setQuery] = useState("");
  const normalized = query.trim().toLowerCase();
  const shown = runs?.filter((run) => !normalized || [run.appId, run.appName, run.releaseRef, run.id, run.status].some((value) => value.toLowerCase().includes(normalized)));
  return (
    <section className="card run-history" aria-label="Pipeline run history">
      <header className="panel-heading">
        <div><h2>pipeline runs</h2><p className="panel-caption">Latest {PIPELINE_RUN_HISTORY_LIMIT} runs in this scope / persisted history is kept after unregistration</p></div>
        <button type="button" className="text-button" disabled={loading} onClick={onRefresh}>{loading ? "loading runs..." : "refresh runs"}</button>
      </header>
      {error && <ErrorBanner error={error} />}
      {runs && runs.length > 0 && (
        <label className="run-history-filter" htmlFor={`${id}-search`}>Filter runs
          <input id={`${id}-search`} type="search" value={query} placeholder="filter these recent runs by app, reference, ID, or status" onChange={(event) => setQuery(event.currentTarget.value)} />
        </label>
      )}
      {runs === null ? <p className="compact-note" role="status">{error ? "Run history could not be loaded. Retry to inspect persisted runs." : "Loading persisted pipeline runs..."}</p> :
        runs.length === 0 ? <p className="compact-note">No pipeline runs recorded. Register a manifest, review its commands, and explicitly start a run.</p> :
          !shown?.length ? <p className="compact-note">No runs match this filter.</p> : (
            <div className="run-history-scroll" tabIndex={0} role="region" aria-label="Recorded runs table">
              <table className="run-history-table">
                <thead><tr><th scope="col">candidate / run</th><th scope="col">outcome</th><th scope="col">stages / commands</th><th scope="col">started / duration</th><th scope="col"><span className="sr-only">Inspect</span></th></tr></thead>
                <tbody>{shown.map((run) => {
                  const app = apps.find((item) => item.manifest.appId === run.appId);
                  return (
                    <tr key={run.id}>
                      <th scope="row">
                        <strong>{run.appName} / {run.releaseRef}</strong><code>{run.id}</code>
                        {!registrationsLoaded ? <span className="run-history-context">registration unavailable / frozen snapshot</span> : !app ? <span className="run-history-context">app unregistered</span> : !matchesRunRegistration(run, app) ? <span className="run-history-context">different registration / frozen snapshot</span> : null}
                      </th>
                      <td><RunStatusBadge status={run.status} />{run.message && <p className="run-history-message">{run.message}</p>}</td>
                      <td>{run.stages.filter((stage) => stage.status === "passed").length}/{run.stages.length} stages passed<br />{run.passedCommands}/{run.totalCommands} commands passed</td>
                      <td><time dateTime={run.startedAt}>{run.startedAt}</time><br />{runDuration(run.startedAt, run.finishedAt)}</td>
                      <td><button type="button" className="button" aria-label={`inspect run ${run.id}`} onClick={() => onSelect(run.id)}>inspect <Arrow /></button></td>
                    </tr>
                  );
                })}</tbody>
              </table>
            </div>
          )}
    </section>
  );
}
