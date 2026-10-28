import { latestConsumerDeployment } from "../lib/consumerDeployments";
import type { ConsumerDeployment, ConsumerSummary, StageSummary } from "../lib/types";
import { Arrow, Device, StatusIcon } from "./Icons";
import "./ConsumerDeployments.css";

export function ConsumerRoutes({
  stages,
  selectedStage,
  onSelectStage,
}: {
  stages: readonly StageSummary[];
  selectedStage: string;
  onSelectStage(stageId: string): void;
}) {
  const routes = stages.flatMap((stage) => stage.consumers.map((consumer) => ({ stage, consumer })));
  if (routes.length === 0) return null;
  return (
    <div className="consumer-routes" role="group" aria-label="Pipeline consumer destinations">
      <span className="eyebrow">consumers / manual delivery</span>
      <div className="consumer-route-list">
        {routes.map(({ stage, consumer }) => (
          <button
            type="button"
            className={`consumer-route ${selectedStage === stage.id ? "active" : ""}`}
            key={`${stage.id}:${consumer.id}`}
            onClick={() => onSelectStage(stage.id)}
            aria-label={`Inspect ${consumer.name} consumer of ${stage.id}`}
          >
            <span>{stage.id}</span><Arrow /><Device /><strong>{consumer.name}</strong>
          </button>
        ))}
      </div>
    </div>
  );
}

export function ConsumerPanel({
  appId,
  stage,
  records,
  onRecord,
  disabled = false,
  preview = false,
  panelRef,
}: {
  appId: string;
  stage: StageSummary;
  records: readonly ConsumerDeployment[];
  onRecord?(consumer: ConsumerSummary): void;
  disabled?: boolean;
  preview?: boolean;
  panelRef?: Ref<HTMLElement>;
}) {
  return (
    <section className="card consumer-panel" aria-label={`${stage.id} consumers`} ref={panelRef} tabIndex={-1}>
      <header className="consumer-panel-heading">
        <h2 className="eyebrow"><Device />consumer destinations</h2>
        <span className="consumer-approval">manual approval</span>
      </header>
      {stage.consumers.length === 0 ? (
        <p className="consumer-empty">No consumers declared for this stage. Add destinations to its <code>consumers</code> manifest entry.</p>
      ) : (
        <ul className="consumer-list">
          {stage.consumers.map((consumer) => {
            const latest = latestConsumerDeployment(records, appId, stage.id, consumer);
            return (
              <li className="consumer-destination" key={consumer.id}>
                <div className="consumer-name"><Device /><h3>{consumer.name}</h3></div>
                <code className="consumer-target">{consumer.target}</code>
                <div className={`consumer-last-record ${latest ? "recorded" : ""}`}>
                  {latest ? (
                    <><span>{preview ? "last sample record" : "last manual record"}</span><strong>{latest.releaseRef}</strong><time dateTime={latest.recordedAt} title={latest.recordedAt}>{new Date(latest.recordedAt).toLocaleString()}</time></>
                  ) : <span>no deployment recorded to this destination</span>}
                </div>
                <button
                  type="button"
                  className="button full"
                  disabled={!onRecord || disabled}
                  aria-label={`deployed to ${consumer.name}`}
                  title={!onRecord ? "Register this manifest before recording a deployment." : disabled ? "Refresh the registry before recording." : "Manually approve and record what you deployed; no files are copied."}
                  onClick={() => onRecord?.(consumer)}
                >deployed to <Arrow /></button>
              </li>
            );
          })}
        </ul>
      )}
      <p className="consumer-footnote">
        {preview ? "Preview records stay in this simulation only." : !onRecord && stage.consumers.length > 0
          ? "Register this manifest to record deployments."
          : disabled ? "Refresh the registry before recording a deployment." : "A manual history marker, not an automated deployment."}
        {" "}Recording delivery does not advance stages. Promotion criteria may require its marker.
      </p>
    </section>
  );
}

export function DeploymentHistory({
  records,
  registeredAppIds,
  preview = false,
}: {
  records: readonly ConsumerDeployment[];
  registeredAppIds?: ReadonlySet<string>;
  preview?: boolean;
}) {
  if (records.length === 0) {
    return <p className="deployment-history-empty">No deployments recorded. Select a stage consumer and use <strong>deployed to</strong> after you have manually delivered a version.</p>;
  }
  return (
    <ol className="deployment-history" aria-label={preview ? "Sample consumer deployment history" : "Manual consumer deployment history"}>
      {records.map((record) => (
        <li className="deployment-entry" key={record.id}>
          <header className="deployment-entry-header">
            <span className="consumer-approval"><StatusIcon tone="done" />{preview ? "sample approval" : "manual approval recorded"}</span>
            <span>recorded <time dateTime={record.recordedAt} title={record.recordedAt}>{new Date(record.recordedAt).toLocaleString()}</time></span>
          </header>
          <div className="deployment-entry-title"><code>{record.releaseRef}</code><span>deployed to</span><strong>{record.consumer.name}</strong></div>
          <div className="deployment-entry-route">
            <span>{record.appName} / {record.stageId}</span><Arrow /><Device /><code>{record.consumer.target}</code>
            {registeredAppIds && !registeredAppIds.has(record.appId) && <span className="meta-tag">app unregistered</span>}
          </div>
          <details className="deployment-provenance">
            <summary>{preview ? "sample" : "record"} #{record.id} / provenance{record.notes ? " + note" : ""}</summary>
            <dl className="metadata">
              <div><dt>application</dt><dd>{record.appId}</dd></div>
              <div><dt>stage</dt><dd>{record.stageId}</dd></div>
              <div><dt>consumer</dt><dd>{record.consumer.id} / {record.consumer.name}</dd></div>
              <div><dt>destination</dt><dd>{record.consumer.target}</dd></div>
              <div><dt>SHA-256</dt><dd>{record.manifestHash}</dd></div>
              <div><dt>manifest</dt><dd>{record.manifestPath}</dd></div>
              <div><dt>source</dt><dd>{record.sourcePath}</dd></div>
              <div><dt>reviewed revision</dt><dd>{record.registrationUpdatedAt}</dd></div>
              <div><dt>recorded</dt><dd>{record.recordedAt}</dd></div>
            </dl>
            {record.notes && <p className="deployment-note">{record.notes}</p>}
            <p className="panel-caption">{preview ? "Illustrative record only." : "Original destination and manifest metadata are retained, even if the registration changes."} Delivery, artifacts, and stage outcomes were not verified.</p>
          </details>
        </li>
      ))}
    </ol>
  );
}
import type { Ref } from "react";
