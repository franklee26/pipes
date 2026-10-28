import { useEffect, useId, useRef, useState } from "react";
import {
  atlasNextStep,
  atlasPresets,
  getPreviewSnapshot,
  getSampleProject,
  getSamplePromotionReview,
  previewStageIds,
  sampleConsumerDeployments,
  sampleHarnessResults,
  sampleProjects,
  sampleRecordEpoch,
  sampleRegistrationUpdatedAt,
  sampleStorage,
  type AtlasPreset,
  type PreviewProjectId,
  type PreviewStageId,
} from "../lib/conceptPreview";
import type { DeploymentReview } from "../lib/consumerDeployments";
import { promotionReady } from "../lib/promotion";
import { simulateHarnessResult, simulatePromotionApproval, simulatePromotionAssessment } from "../lib/promotionPreview";
import type { ConsumerDeployment, HarnessResult, PromotionApproval, PromotionAssessment } from "../lib/types";
import { ConsumerPanel, ConsumerRoutes, DeploymentHistory } from "./ConsumerDeployments";
import { Arrow, Play, Plus, StatusIcon } from "./Icons";
import { HarnessResultHistory, PromotionApprovalHistory, PromotionPanel } from "./PromotionCriteria";
import { ApprovePromotionDialog, ReportHarnessDialog } from "./PromotionDialogs";
import { RecordDeploymentDialog } from "./RecordDeploymentDialog";
import { StageFlow, type StageTone } from "./StageFlow";
import { TabBar } from "./TabBar";
import { WorkspaceShell, type WorkspaceView } from "./WorkspaceShell";

type DetailTab = "checks" | "artifacts" | "manifest" | "promotion" | "history";

interface SampleApprovalRequest {
  projectId: PreviewProjectId;
  stageId: PreviewStageId;
  assessment: PromotionAssessment;
  advanceAfterApproval: boolean;
}

interface SampleHarnessRequest {
  projectId: PreviewProjectId;
  stageId: PreviewStageId;
  checkId: string;
  releaseRef: string;
}

function isPreviewStage(id: string | undefined): id is PreviewStageId {
  return previewStageIds.some((stage) => stage === id);
}

function StatusTag({ tone, children }: { tone: StageTone; children: string }) {
  return <span className={`status-tag ${tone}`}><StatusIcon tone={tone} status={children} />{children}</span>;
}

export function ConceptPreview({ onExit }: { onExit(): void }) {
  const [view, setView] = useState<WorkspaceView>("pipelines");
  const [projectId, setProjectId] = useState<PreviewProjectId>("atlas");
  const [preset, setPreset] = useState<AtlasPreset>("executing");
  const [selectedStage, setSelectedStage] = useState<PreviewStageId>("gamma");
  const [tab, setTab] = useState<DetailTab>("checks");
  const [selectedArtifact, setSelectedArtifact] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [consumerDeployments, setConsumerDeployments] = useState<ConsumerDeployment[]>(() =>
    sampleConsumerDeployments.map((record) => ({ ...record, consumer: { ...record.consumer } })));
  const [harnessResults, setHarnessResults] = useState<HarnessResult[]>(() => sampleHarnessResults.map((result) => ({ ...result })));
  const [promotionApprovals, setPromotionApprovals] = useState<PromotionApproval[]>([]);
  const [consumerReview, setConsumerReview] = useState<{ review: DeploymentReview; releaseRef: string } | null>(null);
  const [harnessRequest, setHarnessRequest] = useState<SampleHarnessRequest | null>(null);
  const [approvalRequest, setApprovalRequest] = useState<SampleApprovalRequest | null>(null);
  const nextSampleRecord = useRef(0);
  const consumerPanel = useRef<HTMLElement>(null);
  const [consumerFocusRequest, setConsumerFocusRequest] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const tabsId = `preview-details-${useId().replaceAll(":", "")}`;
  const project = getSampleProject(projectId);
  const snapshot = getPreviewSnapshot(projectId, preset);
  const stage = snapshot.stages[selectedStage];
  const configuredStage = project.stages.find((item) => item.id === selectedStage);
  const promotionReview = getSamplePromotionReview(projectId, selectedStage);
  const nextStep = atlasNextStep[preset];
  const projects = sampleProjects.map((item) => ({ project: item, snapshot: getPreviewSnapshot(item.id, preset) }));
  const runningCount = projects.filter((item) => item.snapshot.tone === "running").length;
  const gateCount = projects.filter((item) => item.snapshot.tone === "waiting").length;
  const failedCount = projects.filter((item) => item.snapshot.tone === "failed").length;
  const liveCount = projects.filter((item) => item.snapshot.tone === "live").length;
  const dialogOpen = Boolean(consumerReview || harnessRequest || approvalRequest);

  useEffect(() => {
    if (!dialogOpen) heading.current?.focus();
  }, [view, projectId, dialogOpen]);

  useEffect(() => {
    if (consumerFocusRequest > 0) consumerPanel.current?.focus();
  }, [consumerFocusRequest]);

  function navigate(next: WorkspaceView) {
    setNotice(null);
    setView(next);
  }

  function selectProject(id: PreviewProjectId) {
    setProjectId(id);
    setSelectedStage(getPreviewSnapshot(id, preset).selectedStage);
    setSelectedArtifact(null);
    setNotice(null);
  }

  function openRelease(id: PreviewProjectId, stageId?: string) {
    if (stageId !== undefined && !isPreviewStage(stageId)) {
      setNotice(`The sample stage "${stageId}" is not available.`);
      return;
    }
    selectProject(id);
    if (isPreviewStage(stageId)) setSelectedStage(stageId);
    setTab("checks");
    setView("releases");
  }

  function applyPreset(next: AtlasPreset, message: string) {
    setPreset(next);
    setProjectId("atlas");
    setSelectedStage(getPreviewSnapshot("atlas", next).selectedStage);
    setNotice(message);
  }

  function runPipeline() {
    applyPreset("executing", atlasNextStep.live.notice);
    setSelectedArtifact(null);
    setTab("checks");
  }

  function recordMetadata() {
    return {
      id: `preview-${crypto.randomUUID()}`,
      recordedAt: new Date(Date.parse(sampleRecordEpoch) + ++nextSampleRecord.current * 1000).toISOString(),
    };
  }

  function advance() {
    if (preset === "live") {
      runPipeline();
      return;
    }
    if (preset === "gate") {
      const source = getSamplePromotionReview("atlas", "gamma");
      const assessment = simulatePromotionAssessment(source, `v${getSampleProject("atlas").version}`, harnessResults, consumerDeployments);
      setProjectId("atlas");
      setSelectedStage("gamma");
      setView("releases");
      setTab("promotion");
      if (!promotionReady(assessment)) {
        setNotice(`Preview promotion blocked — ${assessment.criteria.filter((criterion) => criterion.status === "blocked").map((criterion) => criterion.message).join(" ")} No sample stage advanced.`);
        return;
      }
      setApprovalRequest({ projectId: "atlas", stageId: "gamma", assessment, advanceAfterApproval: true });
      setNotice("Preview only — review the criteria and explicitly acknowledge any optional skips before the visual scenario advances. No real stage will advance.");
      return;
    }
    applyPreset(nextStep.next, nextStep.notice);
  }

  function transition(id: PreviewProjectId, from: string, to: string) {
    const path = getPreviewSnapshot(id, preset).transitions.find((edge) => edge.from === from && edge.to === to);
    if (id !== "atlas" || !path || (path.tone !== "promoting" && path.tone !== "next")) {
      setNotice("That sample connection cannot be advanced in its current state.");
      return;
    }
    advance();
    if (preset !== "gate") setTab("checks");
    setView("releases");
  }

  function selectArtifacts(name = project.artifacts[0]?.name, focusTab = true) {
    setTab("artifacts");
    setSelectedArtifact(name ?? null);
    setNotice(`Preview only — ${name ? `${name} selected` : "no sample artifact selected"}. No file was fetched or downloaded.`);
    if (focusTab) document.getElementById(`${tabsId}-tab-artifacts`)?.focus();
  }

  const tabs: { id: DetailTab; label: string }[] = [
    { id: "checks", label: `checks (${snapshot.checks.length})` },
    { id: "artifacts", label: `artifacts (${project.artifacts.length})` },
    { id: "manifest", label: "manifest" },
    { id: "promotion", label: "promotion" },
    { id: "history", label: "history" },
  ];

  const manifest = (
    <>
      <p className="panel-caption">Sample {project.id} manifest · illustrative YAML, not read from disk, validated, or registered.</p>
      <pre className="yaml" aria-label={`${project.id} sample manifest`}>{project.manifest}</pre>
    </>
  );

  const inspectorCanAdvance = projectId === "atlas" && (
    (selectedStage === "gamma" && (preset === "executing" || preset === "gate"))
    || (selectedStage === "prod" && preset === "promoted")
  );
  const inspectorBlocked = stage.tone === "idle";
  const inspectorNote = projectId === "beacon"
    ? `Sample only · test failed; prod v${snapshot.currentLive} is retained. No release bundle was produced.`
    : snapshot.currentLive === project.version
      ? `Sample only · prod v${snapshot.currentLive} is live in this illustration, not a real deployment.`
      : `Sample only · prod v${snapshot.currentLive} remains active while candidate v${project.version} advances.`;

  return (
    <>
      <WorkspaceShell
        view={view}
        onNavigate={navigate}
        breadcrumb={view === "releases" ? `${project.id} / v${project.version}` : view}
        connection={{ label: "simulation only", tone: "preview", host: "sample workspace", version: "in-memory / resets on exit" }}
        topbar={(
          <>
            <span className="preview-badge">DESIGN PREVIEW</span>
            <nav className="screen-switch" aria-label="Preview screens">
              <button type="button" aria-current={view === "pipelines" ? "page" : undefined} onClick={() => navigate("pipelines")}>01 / overview</button>
              <button type="button" aria-current={view === "releases" ? "page" : undefined} onClick={() => navigate("releases")}>02 / release</button>
            </nav>
            <button type="button" className="text-button" onClick={onExit}>exit preview</button>
          </>
        )}
        banner={(
          <section className="preview-banner" aria-label="Design preview notice">
            <strong>Design preview · sample data</strong>
            <p>Simulation only. No commands, downloads, or registry changes. Nothing is persisted.</p>
          </section>
        )}
      >
        {notice && (
          <div className="notice">
            <span role="status">{notice}</span>
            <button type="button" aria-label="Dismiss preview notice" onClick={() => { setNotice(null); heading.current?.focus(); }}>&times;</button>
          </div>
        )}

        {view === "pipelines" && (
          <section aria-labelledby="preview-overview-title">
            <div className="page-heading">
              <div>
                <h1 id="preview-overview-title" ref={heading} tabIndex={-1}>pipelines<span className="cursor">_</span></h1>
                <div className="subline">
                  <span className="subline-chip">{sampleProjects.length} sample projects</span>
                  <span className={`subline-chip ${runningCount ? "running" : ""}`}>{runningCount} executing &amp; promoting</span>
                  {gateCount > 0 && <span className="subline-chip">{gateCount} approval gate</span>}
                  <span className="subline-chip">{failedCount} failed pipeline</span>
                  <span className="subline-chip">{liveCount} live {liveCount === 1 ? "candidate" : "candidates"}</span>
                </div>
              </div>
              <div className="actions">
                <button type="button" className="button" onClick={() => navigate("manifest")}><Plus />manifest</button>
                <button type="button" className="button primary" title="Reset the Atlas simulation; no commands run" onClick={runPipeline}><Play />run pipeline</button>
              </div>
            </div>
            <div className="section-bar">
              <h2 className="eyebrow">Active release paths / sample</h2>
              <div className="legend" aria-label="Sample stage legend">
                <span className="text-green"><StatusIcon tone="done" />passed</span>
                <span className="text-orange"><StatusIcon tone="running" />executing / promoting</span>
                <span><StatusIcon tone="waiting" />gate</span>
                <span className="text-red"><StatusIcon tone="failed" />failed</span>
              </div>
            </div>
            <div className="pipeline-list">
              {projects.map(({ project: item, snapshot: release }) => (
                <section className="pipeline-card" key={item.id} aria-label={`${item.id} sample pipeline`}>
                  <header className="pipeline-heading">
                    <div className="project-title">
                      <h2><button type="button" aria-label={`Open ${item.id} sample release`} onClick={() => openRelease(item.id)}>{item.id}</button></h2>
                      <span className="project-version">v{item.version}</span>
                      <span className="project-target">{item.target}</span>
                    </div>
                    <StatusTag tone={release.tone}>{release.status}</StatusTag>
                  </header>
                  <StageFlow
                    stages={item.stages}
                    label={`${item.id} sample pipeline`}
                    variant="overview"
                    onSelectStage={(id) => openRelease(item.id, id)}
                    preview={{ stages: release.stages, transitions: release.transitions, onTransition: (from, to) => transition(item.id, from, to) }}
                  />
                </section>
              ))}
            </div>
            <section className="activity" aria-labelledby="preview-telemetry-title">
              <h2 id="preview-telemetry-title" className="eyebrow">SAMPLE TELEMETRY</h2>
              <div className="activity-events">
                {projects.map(({ project: item, snapshot: release }) => {
                  const event = release.events[release.events.length - 1]!;
                  return <span key={item.id} className={release.tone === "failed" ? "text-red" : release.tone === "running" || release.tone === "waiting" ? "text-orange" : "text-green"}>
                    <time>{event.time.slice(0, 5)}</time>{item.id} / {event.message}
                  </span>;
                })}
              </div>
            </section>
          </section>
        )}

        {view === "releases" && (
          <section aria-labelledby="preview-release-title">
            <div className="page-heading">
              <div>
                <h1 id="preview-release-title" ref={heading} tabIndex={-1}>{project.id} / v{project.version}</h1>
                <div className="release-meta">
                  <span className="meta-tag">commit {project.commit}</span>
                  <span className="meta-tag">{project.target}</span>
                  <span>sample run #{project.run} · fixed timing snapshot</span>
                  <StatusTag tone={snapshot.tone}>{snapshot.status}</StatusTag>
                </div>
              </div>
              <div className="actions">
                <button type="button" className="button" onClick={() => selectArtifacts()}>fetch artifact</button>
                {projectId === "atlas" && <button type="button" className="button primary" aria-label={`Simulate ${nextStep.label}`} onClick={advance}>{nextStep.label}<Arrow /></button>}
              </div>
            </div>
            <section className="state-machine" aria-label={`${project.id} sample promotion state machine`}>
              <div className="machine-bar">
                <div className="machine-bar-left">
                  <span>PIPELINE STATE MACHINE / v{project.version}</span>
                  {projectId === "atlas" ? (
                    <div className="sim-controls" role="group" aria-label="Sample stage state preset">
                      {atlasPresets.map((item) => <button
                        type="button"
                        key={item.id}
                        className={`sim-btn ${preset === item.id ? "active" : ""}`}
                        aria-pressed={preset === item.id}
                        onClick={() => applyPreset(item.id, `Preview only — Atlas ${getPreviewSnapshot("atlas", item.id).status} visual snapshot selected. No approval was recorded and no execution occurred.`)}
                      >{item.label}</button>)}
                    </div>
                  ) : <span className="foundation-badge">fixed {projectId === "beacon" ? "failed" : "live"} sample</span>}
                </div>
                <span className="selected-hint">{projectId === "atlas" ? `step ${preset === "executing" ? 1 : preset === "gate" ? 2 : preset === "promoted" ? 3 : 4} / 4 · ` : ""}click stage to inspect{projectId === "atlas" && preset !== "live" ? " · click arrow to simulate" : ""}</span>
                <span>Visual scenario presets only — selecting a preset does not record an approval.</span>
              </div>
              <StageFlow
                stages={project.stages}
                selectedStage={selectedStage}
                onSelectStage={(id) => {
                  if (isPreviewStage(id)) setSelectedStage(id);
                  else setNotice(`The sample stage "${id}" is not available.`);
                }}
                label={`${project.id} sample release`}
                variant="detail"
                preview={{ stages: snapshot.stages, transitions: snapshot.transitions, onTransition: (from, to) => transition(projectId, from, to) }}
              />
              <ConsumerRoutes stages={project.stages} selectedStage={selectedStage} onSelectStage={(id) => {
                if (isPreviewStage(id)) {
                  setSelectedStage(id);
                  setConsumerFocusRequest((request) => request + 1);
                }
                else setNotice(`The sample stage "${id}" is not available.`);
              }} />
            </section>
            <div className="release-grid">
              <div className="release-main">
                <section className="card" aria-label={`${project.id} sample release details`}>
                  <TabBar<DetailTab> id={tabsId} label="Sample release details" tabs={tabs} value={tab} onChange={setTab} />
                  <div className="panel-body" id={`${tabsId}-panel-checks`} role="tabpanel" aria-labelledby={`${tabsId}-tab-checks`} tabIndex={0} hidden={tab !== "checks"}>
                    {snapshot.checks.map((check) => (
                      <div className={`evidence-row ${check.tone === "running" ? "running-row" : check.tone === "failed" ? "failed-row" : ""}`} key={check.name}>
                        <StatusIcon tone={check.tone} />
                        <span><strong>{check.name}</strong> · {check.description}</span>
                        <span className="evidence-meta">{check.duration}</span>
                        <span className={`evidence-result ${check.tone}`}>{check.result}</span>
                      </div>
                    ))}
                    <p className="panel-caption">Sample storyboard checks and fixed durations, not reported harness evidence. Use the promotion tab to assess actual sample counts and delivery markers.</p>
                  </div>
                  <div className="panel-body" id={`${tabsId}-panel-artifacts`} role="tabpanel" aria-labelledby={`${tabsId}-tab-artifacts`} tabIndex={0} hidden={tab !== "artifacts"}>
                    <p className="panel-caption">Sample artifact metadata only. Selecting an entry never downloads or verifies a file.</p>
                    {project.artifacts.map((artifact) => (
                      <div className="artifact-row" key={artifact.name}>
                        <div>
                          <strong>{artifact.name}</strong>
                          <div className="artifact-hash">sample sha256:{artifact.hash} · {artifact.description}</div>
                          <button type="button" className="text-button" aria-pressed={selectedArtifact === artifact.name} aria-label={`Select sample ${artifact.name}`} onClick={() => selectArtifacts(artifact.name, false)}>
                            {selectedArtifact === artifact.name ? "selected / sample only" : "select sample"}
                          </button>
                        </div>
                        <span className="artifact-size">{artifact.size}</span>
                      </div>
                    ))}
                  </div>
                  <div className="panel-body" id={`${tabsId}-panel-manifest`} role="tabpanel" aria-labelledby={`${tabsId}-tab-manifest`} tabIndex={0} hidden={tab !== "manifest"}>{manifest}</div>
                  <div id={`${tabsId}-panel-promotion`} role="tabpanel" aria-labelledby={`${tabsId}-tab-promotion`} tabIndex={0} hidden={tab !== "promotion"}>
                    {configuredStage && (
                      <PromotionPanel
                        key={`${projectId}:${selectedStage}`}
                        stage={configuredStage}
                        review={promotionReview}
                        results={harnessResults}
                        deployments={consumerDeployments}
                        initialReleaseRef={`v${project.version}`}
                        preview
                        onAssess={async (request) => {
                          if (request.appId !== promotionReview.appId || request.stageId !== promotionReview.stage.id
                            || request.expectedManifestHash !== promotionReview.manifestHash || request.expectedUpdatedAt !== promotionReview.updatedAt) {
                            throw { code: "registration_changed", message: "The sample configuration changed. Review it again before assessing." };
                          }
                          return simulatePromotionAssessment(promotionReview, request.releaseRef, harnessResults, consumerDeployments);
                        }}
                        onReport={(checkId, releaseRef) => setHarnessRequest({ projectId, stageId: selectedStage, checkId, releaseRef })}
                        onApprove={(assessment) => setApprovalRequest({
                          projectId, stageId: selectedStage, assessment: structuredClone(assessment), advanceAfterApproval: false,
                        })}
                      />
                    )}
                  </div>
                  <div className="panel-body" id={`${tabsId}-panel-history`} role="tabpanel" aria-labelledby={`${tabsId}-tab-history`} tabIndex={0} hidden={tab !== "history"}>
                    <p className="panel-caption">Sample history for {project.id} / v{project.version} · illustrative timestamps, not a real run. Storyboard events are separate from the recorded sample evidence and approvals below.</p>
                    <ol className="history">
                      {snapshot.events.map((event) => <li key={`${event.time}-${event.stage}`}><time>{event.time}</time><span>{event.stage} / {event.message}</span></li>)}
                    </ol>
                    <h3 className="eyebrow consumer-history-heading">consumer deployment markers / sample</h3>
                    <DeploymentHistory records={consumerDeployments.filter((record) => record.appId === projectId)} preview />
                    <h3 className="eyebrow consumer-history-heading">externally reported harness results / sample</h3>
                    <HarnessResultHistory results={harnessResults.filter((result) => result.appId === projectId)} preview />
                    <h3 className="eyebrow consumer-history-heading">recorded promotion decisions / sample</h3>
                    <PromotionApprovalHistory approvals={promotionApprovals.filter((approval) => approval.assessment.appId === projectId)} preview />
                  </div>
                </section>
                <details className="terminal" open aria-label={`${project.id} sample execution stream`}>
                  <summary>run #{project.run} · sample execution stream<span>STATIC PREVIEW</span></summary>
                  <div className="terminal-body">
                    {snapshot.events.map((event) => <p key={`${event.time}-${event.stage}`}>
                      <span className="terminal-label">{event.time} [{event.stage}]</span>{" "}
                      <span className={event.tone === "running" || event.tone === "waiting" || event.tone === "failed" ? "terminal-highlight" : undefined}>{event.message}</span>
                    </p>)}
                    <p className="terminal-label">Sample output only — use the controls to advance; no process is running.</p>
                  </div>
                </details>
              </div>
              <aside className="detail-sidebar" aria-label="Selected sample stage">
                <section className="card inspector" aria-labelledby="preview-inspector-title">
                  <p className="eyebrow">{String(previewStageIds.indexOf(selectedStage) + 1).padStart(2, "0")} / selected sample stage</p>
                  <div className="inspector-title">
                    <h2 id="preview-inspector-title">{selectedStage}</h2>
                    <StatusTag tone={stage.tone}>{stage.status}</StatusTag>
                  </div>
                  <dl className="inspector-fields">
                    <div><dt>candidate</dt><dd>{project.id} v{project.version}</dd></div>
                    <div><dt>summary</dt><dd>{stage.summary}</dd></div>
                    <div><dt>timing</dt><dd>{stage.duration} (sample)</dd></div>
                    <div><dt>progress</dt><dd>{stage.progress === null ? "not started" : `${stage.progress}%`}</dd></div>
                    <div><dt>activity</dt><dd>{stage.detail}</dd></div>
                    <div><dt>current live</dt><dd>v{snapshot.currentLive} (sample)</dd></div>
                  </dl>
                  <button
                    type="button"
                    className="button primary full"
                    disabled={inspectorBlocked}
                    onClick={inspectorCanAdvance ? advance : () => selectArtifacts()}
                  >
                    {inspectorCanAdvance ? `simulate: ${nextStep.label}` : inspectorBlocked ? stage.detail : "view sample artifacts"}
                    {!inspectorBlocked && <Arrow />}
                  </button>
                  <p className="inspector-note">{inspectorNote}</p>
                </section>
                {configuredStage && (
                  <ConsumerPanel
                    appId={projectId}
                    stage={configuredStage}
                    records={consumerDeployments}
                    preview
                    panelRef={consumerPanel}
                    onRecord={(consumer) => setConsumerReview({
                      review: {
                        appId: project.id,
                        appName: project.id,
                        stageId: configuredStage.id,
                        consumer: { ...consumer },
                        manifestHash: project.manifestHash,
                        manifestPath: `/preview/${project.id}/pipes.yaml`,
                        sourcePath: `/preview/${project.id}`,
                        updatedAt: sampleRegistrationUpdatedAt,
                      },
                      releaseRef: `v${project.version}`,
                    })}
                  />
                )}
                <details className="receipt" open>
                  <summary>sample cryptographic receipt</summary>
                  <dl>
                    <dt>source</dt><dd>{project.commit}</dd>
                    <dt>manifest</dt><dd>{project.manifestHash}</dd>
                    <dt>{projectId === "beacon" ? "test report" : "artifact"}</dt><dd>{project.artifacts[0]?.hash ?? "none"}</dd>
                    <dt>signature</dt><dd>{project.signature}</dd>
                  </dl>
                  <p className="compact-note">Illustrative only · no signature or file verified.</p>
                </details>
                {projectId !== "atlas" && <button type="button" className="button" onClick={() => openRelease("atlas")}>try Atlas state presets<Arrow /></button>}
              </aside>
            </div>
          </section>
        )}

        {view === "manifest" && (
          <section aria-labelledby="preview-manifest-title">
            <div className="page-heading">
              <div>
                <h1 id="preview-manifest-title" ref={heading} tabIndex={-1}>sample manifest / {project.id}</h1>
                <div className="release-meta"><span className="meta-tag">v{project.version}</span><span>{project.target} · illustrative configuration</span></div>
              </div>
              <button type="button" className="button" onClick={() => openRelease(projectId)}>view sample release<Arrow /></button>
            </div>
            <section className="card" aria-label="Sample manifest library">
              <div className="panel-heading">
                <h2 className="eyebrow">Sample project manifests</h2>
                <div className="sim-controls" role="group" aria-label="Choose sample manifest">
                  {sampleProjects.map((item) => <button type="button" className="sim-btn" aria-pressed={projectId === item.id} key={item.id} onClick={() => selectProject(item.id)}>{item.id}</button>)}
                </div>
              </div>
              <div className="panel-body">{manifest}</div>
            </section>
            <p className="compact-note">The + manifest action opens this sample library, not a file picker. Exit preview to use the real registry.</p>
          </section>
        )}

        {view === "storage" && (
          <section aria-labelledby="preview-storage-title">
            <div className="page-heading">
              <div>
                <h1 id="preview-storage-title" ref={heading} tabIndex={-1}>sample storage</h1>
                <div className="subline"><span className="subline-chip">{sampleStorage.releases} sample releases</span><span className="subline-chip">{sampleStorage.artifacts} sample artifacts</span><span className="subline-chip">0 bytes written</span></div>
              </div>
            </div>
            <section className="card storage-card" aria-label="Sample storage summary">
              <h2 className="eyebrow">Illustrated store / not your local cache</h2>
              <dl className="metadata">
                <div><dt>entries</dt><dd>{sampleStorage.releases} releases / {sampleStorage.artifacts} artifact metadata entries</dd></div>
                <div><dt>sample size</dt><dd>{sampleStorage.megabytes} MB represented · no files stored or downloaded</dd></div>
                <div><dt>persistence</dt><dd>None. All preview changes reset when you exit.</dd></div>
                <div><dt>manual records</dt><dd>{consumerDeployments.length} sample deployment markers / memory only</dd></div>
                <div><dt>criteria records</dt><dd>{harnessResults.length} sample harness reports / {promotionApprovals.length} sample approvals / memory only</dd></div>
                <div><dt>stage heads</dt><dd>Sample version labels only; no real release is pinned or protected.</dd></div>
              </dl>
              {projects.map(({ project: item, snapshot: release }) => (
                <div className="artifact-row" key={item.id}>
                  <div><strong>{item.id} / v{item.version}</strong><p className="compact-note">{item.artifacts.length} sample {item.artifacts.length === 1 ? "artifact" : "artifacts"} · {release.status}</p></div>
                  <button type="button" className="text-button" onClick={() => openRelease(item.id)} aria-label={`Inspect ${item.id} sample release`}>inspect sample<Arrow /></button>
                </div>
              ))}
            </section>
          </section>
        )}

        <footer className="workspace-footer">
          <span>{sampleStorage.releases} sample releases / {sampleStorage.artifacts} sample artifacts / {sampleStorage.megabytes} MB illustrated</span>
          <span>Interactive design preview · fixed timings · no real execution</span>
        </footer>
      </WorkspaceShell>
      {consumerReview && (
        <RecordDeploymentDialog
          review={consumerReview.review}
          initialReleaseRef={consumerReview.releaseRef}
          preview
          onClose={() => setConsumerReview(null)}
          onReturnFocus={() => heading.current?.focus()}
          onConfirm={async (request) => {
            const review = consumerReview.review;
            if (!request.approved || !request.releaseRef.trim()) {
              throw { code: "approval_required", message: "The sample deployment needs a reference and manual approval." };
            }
            return {
              ...recordMetadata(),
              appId: review.appId,
              appName: review.appName,
              stageId: review.stageId,
              consumer: { ...review.consumer },
              releaseRef: request.releaseRef,
              notes: request.notes,
              manifestHash: review.manifestHash,
              manifestPath: review.manifestPath,
              sourcePath: review.sourcePath,
              registrationUpdatedAt: review.updatedAt,
            };
          }}
          onRecorded={(record) => {
            setConsumerDeployments((records) => [record, ...records]);
            setConsumerReview(null);
            setTab("history");
            setNotice(`Preview only — recorded ${record.releaseRef} deployed to ${record.consumer.name}. No registry changes or deployment commands.`);
          }}
        />
      )}
      {harnessRequest && (
        <ReportHarnessDialog
          review={getSamplePromotionReview(harnessRequest.projectId, harnessRequest.stageId)}
          checkId={harnessRequest.checkId}
          initialReleaseRef={harnessRequest.releaseRef}
          preview
          onClose={() => setHarnessRequest(null)}
          onReturnFocus={() => heading.current?.focus()}
          onConfirm={async (request) => simulateHarnessResult(
            getSamplePromotionReview(harnessRequest.projectId, harnessRequest.stageId),
            request,
            recordMetadata(),
          )}
          onRecorded={(result) => {
            setHarnessResults((results) => [result, ...results]);
            setHarnessRequest(null);
            setNotice(`Preview only — ${result.passed}/${result.total} reported for ${result.appId} / ${result.stageId} / ${result.releaseRef}. No harness ran; assess readiness again.`);
          }}
        />
      )}
      {approvalRequest && (
        <ApprovePromotionDialog
          assessment={approvalRequest.assessment}
          preview
          onClose={() => setApprovalRequest(null)}
          onReturnFocus={() => heading.current?.focus()}
          onConfirm={async (request) => {
            if (approvalRequest.advanceAfterApproval && (preset !== "gate"
              || approvalRequest.projectId !== "atlas" || approvalRequest.stageId !== "gamma"
              || request.releaseRef !== `v${getSampleProject("atlas").version}`)) {
              throw { code: "assessment_changed", message: "The visual simulation changed. Request promotion again from the approval gate." };
            }
            return simulatePromotionApproval(
              getSamplePromotionReview(approvalRequest.projectId, approvalRequest.stageId),
              request,
              harnessResults,
              consumerDeployments,
              recordMetadata(),
            );
          }}
          onRecorded={(approval) => {
            setPromotionApprovals((approvals) => [approval, ...approvals]);
            setApprovalRequest(null);
            if (approvalRequest.advanceAfterApproval) {
              applyPreset("promoted", atlasNextStep.gate.notice);
              setTab("checks");
            } else {
              setTab("history");
              setNotice("Preview only — promotion-criteria decision recorded in memory. The sample stage is unchanged; no real stage advances.");
            }
          }}
        />
      )}
    </>
  );
}
