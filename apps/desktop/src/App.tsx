import { useEffect, useRef, useState } from "react";
import { ConceptPreview } from "./components/ConceptPreview";
import { DeploymentHistory } from "./components/ConsumerDeployments";
import { ErrorBanner } from "./components/ErrorBanner";
import { RunBudget } from "./components/CheckPlan";
import { Arrow, Play, Plus, StatusIcon } from "./components/Icons";
import { ManifestPlan } from "./components/ManifestPlan";
import { Modal } from "./components/Modal";
import { PipelineRunDetail } from "./components/PipelineRunDetail";
import { PipelineRunHistory } from "./components/PipelineRunHistory";
import { RegisterDialog } from "./components/RegisterDialog";
import { RecordDeploymentDialog } from "./components/RecordDeploymentDialog";
import { HarnessResultHistory, PromotionApprovalHistory } from "./components/PromotionCriteria";
import { ApprovePromotionDialog, ReportHarnessDialog } from "./components/PromotionDialogs";
import { StageFlow } from "./components/StageFlow";
import { StartRunDialog } from "./components/StartRunDialog";
import { WorkspaceShell, type WorkspaceView } from "./components/WorkspaceShell";
import { useRegistry } from "./hooks/useRegistry";
import { usePipelineRuns } from "./hooks/usePipelineRuns";
import { desktopApi, displayError, type DisplayError } from "./lib/api";
import { reviewConsumerDeployment, type DeploymentReview } from "./lib/consumerDeployments";
import { reviewPromotion, type PromotionReview } from "./lib/promotion";
import type { PipesApi, PromotionAssessment, RegisteredApp } from "./lib/types";
import "./App.css";

function UnregisterDialog({
  app,
  api,
  onClose,
  onRemoved,
}: {
  app: RegisteredApp;
  api: PipesApi;
  onClose(): void;
  onRemoved(): void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DisplayError | null>(null);
  async function remove() {
    setBusy(true);
    setError(null);
    try {
      await api.unregisterApp(app.manifest.appId);
      onRemoved();
    } catch (failure) {
      setError(displayError(failure));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal title={`unregister / ${app.manifest.appId}`} busy={busy} onClose={onClose}>
      <p>Remove this registration? Source files and the manifest stay untouched.</p>
      <p className="compact-note">Recorded consumer deployments are retained with their original destination details. Reported results and promotion approvals are also retained.</p>
      <p className="compact-note">Pipeline runs, frozen manifests, and terminal output remain in Releases. Removing a registration does not cancel an active run.</p>
      {error && <ErrorBanner error={error} />}
      <div className="modal-actions">
        <button type="button" className="button" onClick={onClose} disabled={busy}>cancel</button>
        <button type="button" className="button danger" disabled={busy} onClick={() => { void remove(); }}>
          {busy ? "removing..." : "unregister"}
        </button>
      </div>
    </Modal>
  );
}

export default function App({ api = desktopApi }: { api?: PipesApi }) {
  const registry = useRegistry(api);
  const history = usePipelineRuns(api);
  const [selection, setSelection] = useState<{ appId: string; stageId?: string; tab: "plan" | "manifest" | "deployments" | "promotion"; releaseRef?: string } | null>(null);
  const [view, setView] = useState<WorkspaceView>("pipelines");
  const [previewing, setPreviewing] = useState(false);
  const [registerPath, setRegisterPath] = useState<string | null>(null);
  const [unregistering, setUnregistering] = useState<RegisteredApp | null>(null);
  const [deploymentReview, setDeploymentReview] = useState<DeploymentReview | null>(null);
  const [harnessReview, setHarnessReview] = useState<{ review: PromotionReview; checkId: string; releaseRef: string } | null>(null);
  const [promotionReview, setPromotionReview] = useState<PromotionAssessment | null>(null);
  const [runReview, setRunReview] = useState<RegisteredApp | null>(null);
  const [choosingRun, setChoosingRun] = useState(false);
  const [runId, setRunId] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const previewButton = useRef<HTMLButtonElement>(null);
  const wasPreviewing = useRef(false);
  const apps = registry.snapshot?.apps ?? [];
  const deployments = registry.snapshot?.deployments ?? [];
  const harnessResults = registry.snapshot?.harnessResults ?? [];
  const promotionApprovals = registry.snapshot?.promotionApprovals ?? [];
  const selected = apps.find((app) => app.manifest.appId === selection?.appId);
  const appHistory = usePipelineRuns(api, selected?.manifest.appId, Boolean(selected));
  const info = registry.snapshot?.info;

  useEffect(() => {
    if (selection && registry.snapshot && !registry.loading && !selected) {
      setSelection(null);
      setNotice("That registration is no longer in the registry.");
    }
  }, [selection, registry.snapshot, registry.loading, selected]);

  useEffect(() => {
    if (!previewing && !deploymentReview && !harnessReview && !promotionReview && !runReview && !choosingRun) {
      if (wasPreviewing.current) previewButton.current?.focus();
      else heading.current?.focus();
    }
    wasPreviewing.current = previewing;
  }, [view, selection, previewing, runId]);

  function goToApp(appId: string, stageId?: string, tab: "plan" | "manifest" | "deployments" | "promotion" = "plan", releaseRef?: string) {
    setRunId(null);
    setSelection({ appId, stageId, tab, releaseRef });
    setView("pipelines");
  }

  function navigate(next: WorkspaceView) {
    setSelection(null);
    setRunId(null);
    setView(next);
  }

  function inspectRun(id: string) {
    setSelection(null);
    setRunId(id);
    setView("releases");
  }

  function reviewRun(app: RegisteredApp) {
    setChoosingRun(false);
    setRunReview(structuredClone(app));
  }

  function refreshWorkspace() {
    registry.refresh();
    history.refresh();
    appHistory.refresh();
  }

  const titles: Record<WorkspaceView, string> = { pipelines: "pipelines", releases: "releases", manifest: "manifest library", storage: "local store" };
  const title = selected ? selected.manifest.name : runId ? "pipeline run" : titles[view];
  const status = !api.available ? "desktop required" : registry.error ? "connection error" : registry.loading ? "loading" : "registry connected";

  return (
    <>
      <div hidden={previewing}>
        <WorkspaceShell
          view={view}
          onNavigate={navigate}
          breadcrumb={selected ? `${selected.manifest.appId} / configuration` : runId ? `releases / ${runId}` : view}
          connection={{
            label: status,
            tone: api.available && registry.snapshot && !registry.error ? "connected" : "unavailable",
            host: info ? `${info.os} / ${info.arch}` : undefined,
            version: info ? `v${info.appVersion} / registry + runs` : undefined,
          }}
          topbar={
            <>
              <span className="foundation-badge">LOCAL RUNNER</span>
              <button type="button" className="text-button" disabled={!api.available || registry.loading} onClick={refreshWorkspace}>
                {registry.loading && registry.snapshot ? "loading..." : "refresh"}
              </button>
              <button type="button" ref={previewButton} className="preview-link" onClick={() => setPreviewing(true)}>design preview <Arrow /></button>
            </>
          }
        >
          <header className="page-heading">
            <div>
              <h1 ref={heading} tabIndex={-1}>{title}{!selected && <span className="cursor">_</span>}</h1>
              <p className="subline">
                {selected ? (
                  <>
                    <span className="subline-chip">{selected.manifest.appId} / registered snapshot</span>
                    <span className="subline-chip">{selected.manifest.execution.os ?? "any OS"} / {selected.manifest.execution.arch ?? "any arch"}</span>
                    <span className="subline-chip"><StatusIcon tone="idle" />configuration view</span>
                  </>
                ) : (
                  <>
                    <span className="subline-chip">
                      {registry.snapshot ? `${apps.length} ${apps.length === 1 ? "app" : "apps"} / ${history.runs === null ? history.error ? "run history unavailable" : "loading run history" : `${history.runs.length} ${history.runs.length === 100 ? "recent runs" : history.runs.length === 1 ? "run" : "runs"}`}` :
                        registry.loading ? "reading local registry..." : "registry unavailable"}
                    </span>
                    <span className="subline-chip">manifest snapshots</span>
                    {registry.snapshot && <span className="subline-chip">{deployments.length} manual deployment {deployments.length === 1 ? "record" : "records"}</span>}
                    <span className="subline-chip">worktree execution / explicit approval</span>
                  </>
                )}
              </p>
            </div>
            <div className="actions">
              <button type="button" className="button" disabled={!api.available} onClick={() => setRegisterPath("")}>
                <Plus />manifest
              </button>
              <button
                type="button"
                className="button primary"
                disabled={!api.available || !registry.snapshot || registry.loading || Boolean(registry.error) || apps.length === 0}
                title="Review a registered manifest before explicitly approving execution. Registration never runs commands."
                onClick={() => {
                  if (selected) reviewRun(selected);
                  else if (apps.length === 1 && apps[0]) reviewRun(apps[0]);
                  else setChoosingRun(true);
                }}
              >
                <Play />run pipeline
              </button>
            </div>
          </header>

          {registry.error && (
            <>
              <ErrorBanner error={registry.error} />
              {registry.snapshot && <p className="compact-note">Showing the last loaded snapshot. Refresh to retry.</p>}
            </>
          )}
          {notice && (
            <div className="notice" role="status">
              <span>{notice}</span>
              <button type="button" aria-label="Dismiss notice" onClick={() => setNotice(null)}>&times;</button>
            </div>
          )}

          {!registry.snapshot && !registry.error && <p className="loading" role="status">Loading registry...</p>}
          {!api.available && (
            <section className="empty-state browser-welcome">
              <div className="empty-flow" aria-hidden="true"><span /><Arrow /><span /><Arrow /><span /></div>
              <h2>Your pipelines. A clearer view.</h2>
              <p>Open the desktop app to register manifests and explicitly run real pipelines, or explore the interface with an isolated simulation.</p>
              <button type="button" className="button primary" onClick={() => setPreviewing(true)}>open design preview <Arrow /></button>
              <span className="compact-note">Sample data only. Nothing runs or gets saved.</span>
            </section>
          )}
          {registry.snapshot && view === "storage" && (
            <>
              <div className="storage-summary">
                <section className="card stat-card"><span className="eyebrow">registered apps</span><strong>{apps.length}</strong><span>saved manifest snapshots</span></section>
                <section className="card stat-card"><span className="eyebrow">recent pipeline runs</span><strong>{history.runs?.length ?? "—"}</strong><span>latest 100 / persisted execution history</span></section>
                <section className="card stat-card"><span className="eyebrow">manual deployments</span><strong>{deployments.length}</strong><span>immutable history markers</span></section>
                <section className="card stat-card"><span className="eyebrow">storage engine</span><strong>SQLite</strong><span>local registry, runs &amp; output</span></section>
              </div>
              <section className="card storage-card">
                <header className="panel-heading"><h2>registry</h2><span className="status-tag done">local metadata</span></header>
                <dl className="metadata">
                  <div><dt>apps</dt><dd>{apps.length}</dd></div>
                  <div><dt>directory</dt><dd>{registry.snapshot.info.dataDir}</dd></div>
                  <div><dt>database</dt><dd>{registry.snapshot.info.databasePath}</dd></div>
                  <div><dt>harness reports</dt><dd>{harnessResults.length}</dd></div>
                  <div><dt>promotion approvals</dt><dd>{promotionApprovals.length}</dd></div>
                  <div><dt>recent pipeline runs</dt><dd>{history.runs?.length ?? "not loaded"}</dd></div>
                </dl>
                <p className="compact-note">Registered snapshots, pipeline runs and their output, reported harness results, and approval/deployment history persist here. Unregistration preserves history. Source files stay where they are. Artifact storage is not enabled.</p>
              </section>
            </>
          )}
          {api.available && view === "releases" && (runId ? (
            <PipelineRunDetail
              key={runId}
              api={api}
              runId={runId}
              apps={apps}
              registrationsLoaded={Boolean(registry.snapshot)}
              onUpdate={history.put}
              onBack={() => { setRunId(null); history.refresh(); }}
              onShowConfiguration={(appId, stageId, promotion, releaseRef) => goToApp(appId, stageId, promotion ? "promotion" : "plan", releaseRef)}
            />
          ) : (
            <PipelineRunHistory runs={history.runs} loading={history.loading} error={history.error} apps={apps} registrationsLoaded={Boolean(registry.snapshot)} onSelect={inspectRun} onRefresh={history.refresh} />
          ))}
          {registry.snapshot && view === "releases" && !runId && (
            <>
              <div className="section-bar"><h2 className="eyebrow">Manual deployment history</h2><span className="muted">{deployments.length} recorded / kept after unregistration</span></div>
              <section className="card" aria-label="Recorded consumer deployments">
                <DeploymentHistory records={deployments} registeredAppIds={new Set(apps.map((app) => app.manifest.appId))} />
              </section>
              <div className="section-bar promotion-section-heading"><h2 className="eyebrow">Promotion approvals / optional skips</h2><span className="muted">{promotionApprovals.length} decisions</span></div>
              <section className="card" aria-label="Recorded promotion approvals"><PromotionApprovalHistory approvals={promotionApprovals} /></section>
              <div className="section-bar promotion-section-heading"><h2 className="eyebrow">Externally reported harness results</h2><span className="muted">{harnessResults.length} reports</span></div>
              <section className="card" aria-label="Recorded harness results"><HarnessResultHistory results={harnessResults} /></section>
              <p className="compact-note">Deployment and external evidence records are manual history markers, not verified releases. Pipeline commands run only after explicit execution approval; command success does not supply harness counts or bypass promotion policies. Artifact fetching, storage, and signing are not enabled.</p>
              <div className="actions">
                <button type="button" className="button" onClick={() => navigate("pipelines")}>inspect pipelines</button>
                <button type="button" className="button primary" onClick={() => setPreviewing(true)}>preview release states <Arrow /></button>
              </div>
            </>
          )}
          {registry.snapshot && (view === "pipelines" || view === "manifest") && !selected && (
            apps.length === 0 ? (
              <section className="empty-state">
                <div className="empty-flow" aria-hidden="true"><span /><Arrow /><span /><Arrow /><span /></div>
                <h2>No manifests yet.</h2>
                <p>Choose YAML. Review it. Register.</p>
                <button type="button" className="button primary" onClick={() => setRegisterPath("")}>
                  <Plus />register manifest
                </button>
                <span className="compact-note">Nothing executes on registration.</span>
              </section>
            ) : view === "manifest" ? (
              <>
                <div className="section-bar"><h2 className="eyebrow">Registered manifests</h2><span className="muted">read-only snapshots / SHA-256</span></div>
                <div className="manifest-library">
                  {apps.map((app) => (
                    <section className="card" key={app.manifest.appId} aria-label={`${app.manifest.name} manifest`}>
                      <header className="pipeline-heading">
                        <div className="project-title"><h2>{app.manifest.name}</h2><span className="project-version">{app.manifest.appId}</span></div>
                        <span className="status-tag idle">snapshot</span>
                      </header>
                      <dl className="metadata">
                        <div><dt>manifest</dt><dd>{app.manifest.manifestPath}</dd></div>
                        <div><dt>source</dt><dd>{app.manifest.sourcePath}</dd></div>
                        <div><dt>SHA-256</dt><dd>{app.manifest.manifestHash}</dd></div>
                      </dl>
                      <div className="library-actions"><button type="button" className="button" onClick={() => goToApp(app.manifest.appId, undefined, "manifest")}>view {app.manifest.appId} snapshot <Arrow /></button></div>
                    </section>
                  ))}
                </div>
              </>
            ) : (
              <>
                <div className="section-bar">
                  <h2 className="eyebrow">Configured release paths</h2>
                  <div className="legend"><span><StatusIcon tone="idle" />not run</span><span>all-parent joins</span><span>click stage to inspect</span></div>
                </div>
                <div className="pipeline-list">
                  {apps.map((app) => (
                    <section className="pipeline-card" key={app.manifest.appId} aria-label={`${app.manifest.name} pipeline`}>
                      <header className="pipeline-heading">
                        <div className="project-title">
                          <h2><button type="button" onClick={() => goToApp(app.manifest.appId)}>{app.manifest.name}</button></h2>
                          <span className="project-version">{app.manifest.appId}</span>
                          <span className="project-target">{app.manifest.execution.os ?? "any OS"} / {app.manifest.execution.arch ?? "any arch"}</span>
                        </div>
                        <span className="status-tag idle"><StatusIcon tone="idle" />registered configuration</span>
                      </header>
                      <StageFlow stages={app.manifest.stages} label={`${app.manifest.name} stages`} onSelectStage={(stageId) => goToApp(app.manifest.appId, stageId)} />
                      <div className="pipeline-run-actions">
                        <p className="compact-note">Configuration only / inspect run history for recorded outcomes</p>
                        <button type="button" className="button" disabled={registry.loading || Boolean(registry.error)} onClick={() => reviewRun(app)} aria-label={`run ${app.manifest.name} pipeline`}><Play />review &amp; run</button>
                        <button type="button" className="text-button" onClick={() => goToApp(app.manifest.appId)}>configuration &amp; history <Arrow /></button>
                      </div>
                      <div className="pipeline-footer"><span title={app.manifest.sourcePath}>{app.manifest.sourcePath}</span><code>{app.manifest.manifestHash.slice(0, 12)}</code></div>
                    </section>
                  ))}
                </div>
                <section className="activity" aria-label="Registry summary">
                  <h2 className="eyebrow">REGISTRY</h2>
                  <div className="activity-events"><span>{apps.length} saved {apps.length === 1 ? "snapshot" : "snapshots"}</span><span>recent runs: {history.runs?.length ?? "—"} / inspect in Releases</span><span>local / shared with CLI</span></div>
                </section>
              </>
            )
          )}
          {selected && view === "pipelines" && (
            <div className="application-detail">
              <div className="detail-toolbar">
                <button type="button" className="text-button back-link" onClick={() => navigate("pipelines")}><Arrow />all pipelines</button>
                <div className="actions">
                  <button type="button" className="text-button" onClick={() => setRegisterPath(selected.manifest.manifestPath)}>re-read file</button>
                  <button type="button" className="text-button destructive" onClick={() => setUnregistering(selected)}>unregister</button>
                </div>
              </div>
              <ManifestPlan
                key={`${selected.manifest.manifestHash}:${selection?.stageId ?? ""}:${selection?.tab ?? "plan"}:${selection?.releaseRef ?? ""}`}
                manifest={selected.manifest}
                initialStage={selection?.stageId}
                initialTab={selection?.tab}
                showManifest
                deployments={deployments.filter((record) => record.appId === selected.manifest.appId)}
                onRecordConsumer={(stageId, consumer) => setDeploymentReview(reviewConsumerDeployment(selected, stageId, consumer))}
                recordingDisabled={registry.loading || Boolean(registry.error)}
                registration={selected}
                harnessResults={harnessResults}
                promotionApprovals={promotionApprovals}
                initialReleaseRef={selection?.releaseRef}
                onAssessPromotion={(request) => api.assessPromotion(request)}
                onReportHarness={(stage, checkId, releaseRef) => {
                  if (!stage.checks.some((check) => check.id === checkId)) {
                    setNotice("That harness is no longer configured. Refresh and review the current manifest.");
                    registry.refresh();
                    return;
                  }
                  setHarnessReview({ review: reviewPromotion(selected, stage), checkId, releaseRef });
                }}
                onApprovePromotion={setPromotionReview}
              />
              <details className="disclosure">
                <summary>registration</summary>
                <dl className="metadata">
                  <div><dt>manifest</dt><dd>{selected.manifest.manifestPath}</dd></div>
                  <div><dt>source</dt><dd>{selected.manifest.sourcePath}</dd></div>
                  <div><dt>version</dt><dd>semver / {selected.manifest.versionFrom}</dd></div>
                  <div><dt>host</dt><dd>{selected.manifest.execution.os ?? "any"} / {selected.manifest.execution.arch ?? "any"}</dd></div>
                  <div><dt>parallel</dt><dd><RunBudget value={selected.manifest.execution.maxParallel} /></dd></div>
                  <div><dt>registered</dt><dd>{selected.registeredAt}</dd></div>
                  <div><dt>updated</dt><dd>{selected.updatedAt}</dd></div>
                </dl>
              </details>
              <div className="application-run-history">
                <PipelineRunHistory runs={appHistory.runs} loading={appHistory.loading} error={appHistory.error} apps={apps} onSelect={inspectRun} onRefresh={appHistory.refresh} />
              </div>
            </div>
          )}
          <footer className="workspace-footer"><span>local state / shared Rust core</span><span>execution: local commands / artifact signing: not enabled</span></footer>
        </WorkspaceShell>

        {choosingRun && (
          <Modal title="choose a pipeline to review" onClose={() => setChoosingRun(false)}>
            <p>Choose a registered configuration. Nothing executes until you review and explicitly approve its commands.</p>
            <div className="run-picker">{apps.map((app) => <button type="button" className="button" key={app.manifest.appId} onClick={() => reviewRun(app)}>review {app.manifest.name} / {app.manifest.appId} <Arrow /></button>)}</div>
          </Modal>
        )}
        {runReview && (
          <StartRunDialog
            review={runReview}
            onClose={() => setRunReview(null)}
            onConfirm={(request) => api.startPipelineRun(request)}
            onInvalidated={refreshWorkspace}
            onReturnFocus={() => heading.current?.focus()}
            onStarted={(run) => {
              history.put(run);
              history.refresh();
              setRunReview(null);
              setNotice(`Started ${run.appName} / ${run.releaseRef}. Commands run against the current working tree.`);
              inspectRun(run.id);
            }}
          />
        )}
        {registerPath !== null && (
          <RegisterDialog
            api={api}
            apps={apps}
            initialPath={registerPath}
            onClose={() => setRegisterPath(null)}
            onRefresh={registry.refresh}
            onRegistered={(app) => {
              registry.put(app);
              setRegisterPath(null);
              setNotice(`${app.manifest.name} registered. No commands ran.`);
              goToApp(app.manifest.appId);
            }}
          />
        )}
        {unregistering && (
          <UnregisterDialog
            app={unregistering}
            api={api}
            onClose={() => setUnregistering(null)}
            onRemoved={() => {
              registry.remove(unregistering.manifest.appId);
              setUnregistering(null);
              setNotice("Registration removed. Source files unchanged. Deployment history retained.");
              navigate("pipelines");
            }}
          />
        )}
        {deploymentReview && (
          <RecordDeploymentDialog
            review={deploymentReview}
            onClose={() => setDeploymentReview(null)}
            onConfirm={(request) => api.recordConsumerDeployment(request)}
            onInvalidated={registry.refresh}
            onReturnFocus={() => heading.current?.focus()}
            onRecorded={(record) => {
              registry.putDeployment(record);
              setDeploymentReview(null);
              setNotice(`Recorded ${record.releaseRef} deployed to ${record.consumer.name}. No deployment command ran.`);
              goToApp(record.appId, record.stageId, "deployments", record.releaseRef);
            }}
          />
        )}
        {harnessReview && (
          <ReportHarnessDialog
            review={harnessReview.review}
            checkId={harnessReview.checkId}
            initialReleaseRef={harnessReview.releaseRef}
            onClose={() => setHarnessReview(null)}
            onConfirm={(request) => api.recordHarnessResult(request)}
            onInvalidated={registry.refresh}
            onReturnFocus={() => heading.current?.focus()}
            onRecorded={(result) => {
              registry.putHarnessResult(result);
              setHarnessReview(null);
              setNotice(`Reported ${result.passed}/${result.total} for ${result.checkId}. Reporting does not run commands or verify the supplied counts.`);
              goToApp(result.appId, result.stageId, "promotion", result.releaseRef);
            }}
          />
        )}
        {promotionReview && (
          <ApprovePromotionDialog
            assessment={promotionReview}
            onClose={() => setPromotionReview(null)}
            onConfirm={(request) => api.approvePromotion(request)}
            onInvalidated={registry.refresh}
            onReturnFocus={() => heading.current?.focus()}
            onRecorded={(approval) => {
              registry.putPromotionApproval(approval);
              setPromotionReview(null);
              setNotice(approval.skippedOptional.length
                ? `Promotion criteria approved with skipped optional steps: ${approval.skippedOptional.join(", ")}. A matching waiting run may now continue.`
                : "Promotion criteria approved. A matching waiting run may now continue; no new run was started.");
              history.refresh();
              goToApp(approval.assessment.appId, approval.assessment.stageId, "promotion", approval.assessment.releaseRef);
            }}
          />
        )}
      </div>
      {previewing && (
        <ConceptPreview onExit={() => setPreviewing(false)} />
      )}
    </>
  );
}
