import { useEffect, useId, useRef, useState } from "react";
import { deploymentsForStage } from "../lib/consumerDeployments";
import { harnessResultsForStage, reviewPromotion } from "../lib/promotion";
import type { ConsumerDeployment, ConsumerSummary, HarnessResult, ManifestPreview, PromotionApproval, PromotionAssessment, PromotionContext, RegisteredApp, StageSummary } from "../lib/types";
import { CheckPlan, RunBudget } from "./CheckPlan";
import { CommandList } from "./CommandList";
import { ConsumerPanel, ConsumerRoutes, DeploymentHistory } from "./ConsumerDeployments";
import { Arrow, StatusIcon } from "./Icons";
import { StageFlow } from "./StageFlow";
import { HarnessResultHistory, PromotionApprovalHistory, PromotionPanel } from "./PromotionCriteria";
import { TabBar, type Tab } from "./TabBar";

type PlanTab = "plan" | "manifest" | "outputs" | "deployments" | "promotion";

export function ManifestPlan({
  manifest,
  initialStage,
  initialTab = "plan",
  showManifest = false,
  deployments,
  onRecordConsumer,
  recordingDisabled = false,
  registration,
  harnessResults = [],
  promotionApprovals = [],
  initialReleaseRef,
  onAssessPromotion,
  onReportHarness,
  onApprovePromotion,
}: {
  manifest: ManifestPreview;
  initialStage?: string;
  initialTab?: PlanTab;
  showManifest?: boolean;
  deployments?: readonly ConsumerDeployment[];
  onRecordConsumer?(stageId: string, consumer: ConsumerSummary): void;
  recordingDisabled?: boolean;
  registration?: RegisteredApp;
  harnessResults?: readonly HarnessResult[];
  promotionApprovals?: readonly PromotionApproval[];
  initialReleaseRef?: string;
  onAssessPromotion?(request: PromotionContext): Promise<PromotionAssessment>;
  onReportHarness?(stage: StageSummary, checkId: string, releaseRef: string): void;
  onApprovePromotion?(assessment: PromotionAssessment): void;
}) {
  const [stageId, setStageId] = useState(initialStage);
  const [tab, setTab] = useState<PlanTab>(initialTab);
  const tabId = useId();
  const consumerPanel = useRef<HTMLElement>(null);
  const [consumerFocusRequest, setConsumerFocusRequest] = useState(0);
  const stage = manifest.stages.find((item) => item.id === stageId) ?? manifest.stages[0];
  const checkCount = manifest.stages.reduce((count, item) => count + item.checks.length, 0);
  const stageDeployments = stage ? deploymentsForStage(deployments ?? [], manifest.appId, stage.id) : [];
  const stageResults = stage ? harnessResultsForStage(harnessResults, manifest.appId, stage.id) : [];
  const stageApprovals = promotionApprovals.filter((approval) => approval.assessment.appId === manifest.appId && approval.assessment.stageId === stage?.id);
  const hasPromotionTab = Boolean(stage && (stage.promotionCriteria.length > 0 || stageResults.length > 0 || stageApprovals.length > 0));
  const tabs: Tab<PlanTab>[] = [
    { id: "plan", label: "plan" },
    ...(showManifest ? [{ id: "manifest" as const, label: "manifest" }] : []),
    { id: "outputs", label: `outputs (${manifest.outputs.length})` },
    ...(deployments ? [{ id: "deployments" as const, label: `deployments (${stageDeployments.length})` }] : []),
    ...(hasPromotionTab ? [{ id: "promotion" as const, label: "promotion" }] : []),
  ];

  useEffect(() => {
    if (consumerFocusRequest > 0) consumerPanel.current?.focus();
  }, [consumerFocusRequest]);

  return (
    <>
      <section className="state-machine" aria-label="Pipeline plan">
        <div className="machine-bar">
          <h2 className="eyebrow">Pipeline state machine / configured</h2>
          <span className="selected-hint">click stage to inspect</span>
        </div>
        <StageFlow
          stages={manifest.stages}
          selectedStage={stage?.id}
          onSelectStage={setStageId}
          variant="detail"
        />
        {stage && <ConsumerRoutes stages={manifest.stages} selectedStage={stage.id} onSelectStage={(id) => {
          setStageId(id);
          setConsumerFocusRequest((request) => request + 1);
        }} />}
      </section>
      {stage && (
        <section className="release-grid" aria-label={`${stage.id} configuration`}>
          <div className="release-main">
            <section className="card stage-detail" aria-label="Stage details">
              <TabBar<PlanTab> id={tabId} label="Application details" tabs={tabs} value={tab} onChange={setTab} />
              <div id={`${tabId}-panel-plan`} role="tabpanel" aria-labelledby={`${tabId}-tab-plan`} hidden={tab !== "plan"} tabIndex={0}>
                <div className="requirements">
                  <span className="eyebrow">{stage.after.length === 0 ? "entry stage" : stage.after.length > 1 ? "after all" : "after"}</span>
                  {stage.after.map((parent) => <code key={parent}>{parent}</code>)}
                </div>
                <CheckPlan steps={stage.checkPlan} parallelism={manifest.execution.maxParallel} sourcePath={manifest.sourcePath} />
              </div>
              {showManifest && (
                <div id={`${tabId}-panel-manifest`} role="tabpanel" aria-labelledby={`${tabId}-tab-manifest`} hidden={tab !== "manifest"} tabIndex={0}>
                  <header className="panel-heading"><h2 className="eyebrow">Registered snapshot</h2><span className="muted">read only</span></header>
                  <pre className="yaml">{manifest.manifestYaml}</pre>
                </div>
              )}
              <div id={`${tabId}-panel-outputs`} role="tabpanel" aria-labelledby={`${tabId}-tab-outputs`} hidden={tab !== "outputs"} tabIndex={0} className="panel-body">
                {manifest.outputs.length > 0 ? manifest.outputs.map((output) => (
                  <div className="artifact-row" key={output.name}>
                    <div><strong>{output.name}</strong><div className="artifact-hash">{output.path}</div></div>
                    <span className="meta-tag">{output.format}</span>
                  </div>
                )) : <p className="compact-note">No outputs declared in this manifest.</p>}
                <p className="panel-caption">Declared outputs only. This configuration view does not verify builds or store artifacts.</p>
              </div>
              {deployments && (
                <div id={`${tabId}-panel-deployments`} role="tabpanel" aria-labelledby={`${tabId}-tab-deployments`} hidden={tab !== "deployments"} tabIndex={0}>
                  <DeploymentHistory records={stageDeployments} />
                </div>
              )}
              {hasPromotionTab && (
                <div id={`${tabId}-panel-promotion`} role="tabpanel" aria-labelledby={`${tabId}-tab-promotion`} hidden={tab !== "promotion"} tabIndex={0}>
                  <PromotionPanel
                    key={`${manifest.appId}:${stage.id}:${registration?.updatedAt ?? manifest.manifestHash}`}
                    stage={stage}
                    review={registration ? reviewPromotion(registration, stage) : undefined}
                    results={stageResults}
                    deployments={stageDeployments}
                    initialReleaseRef={initialReleaseRef}
                    onAssess={onAssessPromotion}
                    onReport={onReportHarness ? (checkId, reference) => onReportHarness(stage, checkId, reference) : undefined}
                    onApprove={onApprovePromotion}
                    disabled={recordingDisabled}
                  />
                  <h3 className="eyebrow promotion-history-heading">reported harness evidence</h3>
                  <HarnessResultHistory results={stageResults} />
                  <h3 className="eyebrow promotion-history-heading">promotion approval history</h3>
                  <PromotionApprovalHistory approvals={stageApprovals} />
                </div>
              )}
            </section>
            <details className="terminal" open>
              <summary>execution / configuration view <span>plan only</span></summary>
              <div className="terminal-body">
                <p><span className="terminal-label">[manifest]</span> {manifest.stages.length} configured stages / {checkCount} planned {checkCount === 1 ? "check" : "checks"}</p>
                <p><span className="terminal-label">[runner]</span> Inspecting or registering a manifest never executes commands. Real run outcomes and terminal output are in Releases.</p>
              </div>
            </details>
            {manifest.prepare.length > 0 && (
              <details className="disclosure">
                <summary>prepare / {manifest.prepare.length} steps</summary>
                <CommandList commands={manifest.prepare} sourcePath={manifest.sourcePath} />
              </details>
            )}
          </div>
          <aside className="detail-sidebar" aria-label="Selected stage">
            <section className="card inspector">
              <p className="eyebrow">{String(manifest.stages.indexOf(stage) + 1).padStart(2, "0")} / selected stage</p>
              <div className="inspector-title"><h2>{stage.id}</h2><span className="status-tag idle"><StatusIcon tone="idle" />not run</span></div>
              <dl className="inspector-fields">
                <div><dt>checks</dt><dd>{stage.checks.length} planned</dd></div>
                <div><dt>approval</dt><dd>{stage.requiresApproval ? "manual gate" : "not required"}</dd></div>
                <div><dt>parallel</dt><dd><RunBudget value={manifest.execution.maxParallel} /></dd></div>
                <div><dt>execution</dt><dd>requires a reviewed pipeline run</dd></div>
                <div><dt>promotion rules</dt><dd>{stage.promotionCriteria.length} declared</dd></div>
              </dl>
              {stage.requires.length > 0 && (
                <div className="inspector-requirements">
                  <span className="eyebrow">declared requirements</span>
                  {stage.requires.map((requirement) => <code key={requirement}>{requirement}</code>)}
                </div>
              )}
              <button type="button" className="button primary full" disabled title="Start a reviewed pipeline run to execute stages in dependency order.">run stage <Arrow /></button>
              {hasPromotionTab && <button type="button" className="button full promotion-inspect-button" onClick={() => {
                setTab("promotion");
                document.getElementById(`${tabId}-tab-promotion`)?.focus();
              }}>promotion criteria <Arrow /></button>}
              <p className="inspector-note">Policies describe intent, not execution evidence.</p>
            </section>
            <ConsumerPanel
              appId={manifest.appId}
              stage={stage}
              records={stageDeployments}
              onRecord={onRecordConsumer ? (consumer) => onRecordConsumer(stage.id, consumer) : undefined}
              disabled={recordingDisabled}
              panelRef={consumerPanel}
            />
            <details className="receipt" open>
              <summary>manifest receipt</summary>
              <dl>
                <dt>app</dt><dd>{manifest.appId}</dd>
                <dt>SHA-256</dt><dd title={manifest.manifestHash}>{manifest.manifestHash.slice(0, 8)}...{manifest.manifestHash.slice(-8)}</dd>
                <dt>version source</dt><dd>{manifest.versionFrom}</dd>
                <dt>artifact</dt><dd>not verified here</dd>
                <dt>signature</dt><dd>not signed</dd>
              </dl>
            </details>
          </aside>
        </section>
      )}
    </>
  );
}
