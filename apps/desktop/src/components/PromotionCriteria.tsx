import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { releaseReferenceProblem } from "../lib/inputValidation";
import { promotionContext, promotionReady, promotionStatusLabel, type PromotionReview } from "../lib/promotion";
import type { ConsumerDeployment, CriterionEvaluation, HarnessResult, PromotionApproval, PromotionAssessment, PromotionContext, PromotionCriterion, StageSummary } from "../lib/types";
import { ErrorBanner } from "./ErrorBanner";
import { Arrow, StatusIcon } from "./Icons";
import "./PromotionCriteria.css";

function criterionTitle(criterion: PromotionCriterion): string {
  return criterion.kind === "harness" ? criterion.checkId : criterion.consumerId;
}

export function CriteriaDefinitions({ stage, onReport }: { stage: StageSummary; onReport?(checkId: string): void }) {
  return (
    <ul className="criteria-list" aria-label="Declared promotion criteria">
      {stage.promotionCriteria.map((criterion) => (
        <li className="criterion-row" key={criterion.id}>
          <div className="criterion-heading">
            <strong>{criterion.id}</strong>
            <span className={`criterion-requirement ${criterion.kind === "deployment" && criterion.requirement === "optional" ? "optional" : ""}`}>
              {criterion.kind === "harness" ? "required harness" : `${criterion.requirement} deployment`}
            </span>
          </div>
          {criterion.kind === "harness" ? (
            <div className="criterion-definition">
              <span><code>{criterion.checkId}</code> / at least <strong>{criterion.minimumPassed}</strong> pass / exactly <strong>{criterion.expectedTests}</strong> tests</span>
              {onReport && <button type="button" className="text-button" onClick={() => onReport(criterion.checkId)} aria-label={`Report results for ${criterion.checkId}`}>report results <Arrow /></button>}
            </div>
          ) : (
            <p className="criterion-definition">
              <code>{criterion.consumerId}</code>
              {" / "}{stage.consumers.find((consumer) => consumer.id === criterion.consumerId)?.target}
              {criterion.requirement === "optional" && <span className="criterion-optional-note">May be skipped only with an acknowledged warning.</span>}
            </p>
          )}
        </li>
      ))}
    </ul>
  );
}

export function AssessmentDetails({ assessment, historical = false }: { assessment: PromotionAssessment; historical?: boolean }) {
  const tone = assessment.status === "ready" ? "done" : assessment.status === "ready_with_warnings" ? "waiting" : "failed";
  return (
    <section className="promotion-assessment" aria-label={historical ? "Recorded promotion assessment" : "Promotion readiness assessment"}>
      <div className={`assessment-heading ${tone}`}>
        <span className={`status-tag ${tone}`}><StatusIcon tone={tone} />{promotionStatusLabel(assessment)}</span>
        <code>{assessment.releaseRef}</code>
      </div>
      <ul className="criteria-results">
        {assessment.criteria.map((result) => <CriterionResult key={result.criterion.id} result={result} />)}
      </ul>
      {assessment.status === "not_configured" && <p className="compact-note">Define a promotion policy before approving a promotion decision.</p>}
      {assessment.status === "ready_with_warnings" && <p className="optional-warning">{historical
        ? "These optional steps were acknowledged as skipped when this approval was recorded. Later evidence does not change this decision."
        : "Promotion approval is possible, but the optional steps above will be recorded as skipped unless you complete them first."}</p>}
    </section>
  );
}

function CriterionResult({ result }: { result: CriterionEvaluation }) {
  const tone = result.status === "satisfied" ? "done" : result.status === "warning" ? "waiting" : "failed";
  return (
    <li className={`criterion-result ${result.status}`}>
      <StatusIcon tone={tone} />
      <div>
        <div className="criterion-heading"><strong>{result.criterion.id}</strong><span>{result.status}</span></div>
        <p>{result.message}</p>
        <div className="criterion-evidence">
          <code>{criterionTitle(result.criterion)}</code>
          {result.observed && <span>{result.observed.passed}/{result.observed.total} reported passes</span>}
          {result.consumer && <span>{result.consumer.name} / {result.consumer.target}</span>}
          {result.evidenceId && <span>evidence #{result.evidenceId}</span>}
        </div>
      </div>
    </li>
  );
}

export function PromotionPanel({
  stage,
  review,
  results = [],
  deployments = [],
  onAssess,
  onReport,
  onApprove,
  initialReleaseRef = "",
  disabled = false,
  preview = false,
}: {
  stage: StageSummary;
  review?: PromotionReview;
  results?: readonly HarnessResult[];
  deployments?: readonly ConsumerDeployment[];
  onAssess?(request: PromotionContext): Promise<PromotionAssessment>;
  onReport?(checkId: string, releaseRef: string): void;
  onApprove?(assessment: PromotionAssessment): void;
  initialReleaseRef?: string;
  disabled?: boolean;
  preview?: boolean;
}) {
  const id = useId();
  const [reference, setReference] = useState(initialReleaseRef);
  const [assessment, setAssessment] = useState<PromotionAssessment | null>(null);
  const [error, setError] = useState<DisplayError | null>(null);
  const [busy, setBusy] = useState(false);
  const request = useRef(0);
  const referenceProblem = releaseReferenceProblem(reference);
  const harnessIds = new Set(stage.promotionCriteria.flatMap((criterion) => criterion.kind === "harness" ? [criterion.checkId] : []));
  const consumerIds = new Set(stage.promotionCriteria.flatMap((criterion) => criterion.kind === "deployment" ? [criterion.consumerId] : []));
  const evidenceKey = JSON.stringify([
    results.filter((result) => result.appId === review?.appId && result.stageId === stage.id && result.releaseRef === reference.trim() && harnessIds.has(result.checkId)).map((result) => result.id),
    deployments.filter((record) => record.appId === review?.appId && record.stageId === stage.id && record.releaseRef === reference.trim() && consumerIds.has(record.consumer.id)).map((record) => record.id),
    review?.manifestHash, review?.updatedAt, stage.id,
  ]);
  useEffect(() => {
    request.current += 1;
    setAssessment(null);
    setBusy(false);
  }, [evidenceKey]);
  useEffect(() => () => { request.current += 1; }, []);

  async function assess(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!review || !onAssess) {
      setError({ code: "registration_required", message: "Register the manifest before assessing real promotion criteria." });
      return;
    }
    if (!reference.trim() || referenceProblem) {
      setError({ code: "invalid_promotion", message: referenceProblem ?? "Enter the candidate release reference." });
      return;
    }
    const version = ++request.current;
    setBusy(true);
    setError(null);
    setAssessment(null);
    try {
      const result = await onAssess(promotionContext(review, reference));
      if (request.current === version) setAssessment(result);
    } catch (failure) {
      if (request.current === version) setError(displayError(failure));
    } finally {
      if (request.current === version) setBusy(false);
    }
  }

  return (
    <section className="promotion-panel" aria-label={`${stage.id} promotion criteria`}>
      <header className="panel-heading"><h2 className="eyebrow">Promotion criteria / {stage.id}</h2><span className="muted">{stage.promotionCriteria.length} rules</span></header>
      {stage.promotionCriteria.length === 0 ? <p className="promotion-empty">No custom promotion criteria are configured. Add a <code>promotion.criteria</code> declaration to this stage.</p> : (
        <CriteriaDefinitions stage={stage} onReport={onReport && !disabled ? (checkId) => onReport(checkId, reference.trim()) : undefined} />
      )}
      {review && onAssess && (
        <form className="promotion-reference-form" onSubmit={(event) => { void assess(event); }}>
          <label htmlFor={`${id}-reference`}>Candidate release reference</label>
          <div className="promotion-reference-field">
            <input id={`${id}-reference`} value={reference} required maxLength={512} placeholder="Version, commit, or artifact reference" autoComplete="off" spellCheck={false} aria-invalid={Boolean(referenceProblem)} disabled={disabled || busy} onChange={(event) => {
              request.current += 1;
              setReference(event.currentTarget.value);
              setAssessment(null);
              setError(null);
            }} />
            <button type="submit" className="button" disabled={disabled || busy || !reference.trim() || Boolean(referenceProblem)}>
              {busy ? "assessing..." : "assess readiness"}
            </button>
          </div>
          {referenceProblem && <p className="deployment-field-error" role="status">{referenceProblem}</p>}
          <p className="panel-caption">Only evidence for this reference and the current registered configuration counts. Missing results are never a pass.</p>
        </form>
      )}
      {error && <ErrorBanner error={error} />}
      {assessment && (
        <>
          <AssessmentDetails assessment={assessment} />
          {onApprove && <div className="promotion-approve-action"><button type="button" className="button primary" disabled={disabled || !promotionReady(assessment)} onClick={() => onApprove(assessment)}>
            {assessment.status === "ready_with_warnings" ? "review optional skips & approve" : "review promotion approval"}<Arrow />
          </button></div>}
        </>
      )}
      <div className="promotion-scope-note">
        {preview ? "Simulation only. " : !review ? "Register this manifest to report results and evaluate real evidence. " : ""}
        This assesses declared promotion criteria only. {preview
          ? "Approval records a sample decision; it does not run commands, advance a stage, or verify its other requirements."
          : "Approval records a policy decision; it does not start a run or verify other stage requirements. An already approved run waiting on this candidate and registration may continue after the runner rechecks the decision. approval.local still requires separate per-run consent."}
        {stage.requires.length > 0 && <div className="requirements"><span className="eyebrow">other stage requirements / not evaluated</span>{stage.requires.map((condition) => <code key={condition}>{condition}</code>)}</div>}
      </div>
    </section>
  );
}

export function HarnessResultHistory({ results, preview = false }: { results: readonly HarnessResult[]; preview?: boolean }) {
  if (results.length === 0) return <p className="promotion-empty">No harness results reported. Command outcomes do not supply or verify test counts.</p>;
  return <ol className="harness-history" aria-label={preview ? "Sample reported harness results" : "Reported harness results"}>
    {results.map((result) => <li key={result.id}>
      <div className="criterion-heading"><strong>{result.checkId}</strong><code>{result.releaseRef}</code><span>{result.passed}/{result.total} reported passes</span></div>
      <span className="muted">{result.appName} / {result.stageId} / reported <time dateTime={result.recordedAt}>{new Date(result.recordedAt).toLocaleString()}</time></span>
      <details className="deployment-provenance"><summary>{preview ? "sample" : "report"} #{result.id} / provenance</summary>
        <dl className="metadata">
          <div><dt>manifest hash</dt><dd>{result.manifestHash}</dd></div>
          <div><dt>manifest</dt><dd>{result.manifestPath}</dd></div>
          <div><dt>source</dt><dd>{result.sourcePath}</dd></div>
          <div><dt>revision</dt><dd>{result.registrationUpdatedAt}</dd></div>
        </dl>
        {result.notes && <p className="deployment-note">{result.notes}</p>}
        <p className="panel-caption">Externally supplied counts, not a harness executed or verified by Pipes.</p>
      </details>
    </li>)}
  </ol>;
}

export function PromotionApprovalHistory({ approvals, preview = false }: { approvals: readonly PromotionApproval[]; preview?: boolean }) {
  if (approvals.length === 0) return <p className="promotion-empty">No promotion-criteria approvals recorded.</p>;
  return <ol className="promotion-approval-history" aria-label={preview ? "Sample promotion approval history" : "Promotion approval history"}>
    {approvals.map((approval) => <li className={approval.skippedOptional.length ? "with-skips" : ""} key={approval.id}>
      <div className="criterion-heading">
        <strong>{approval.assessment.appName} / {approval.assessment.stageId}</strong>
        <span className={`status-tag ${approval.skippedOptional.length ? "waiting" : "done"}`}>{approval.skippedOptional.length ? "approved with optional skips" : "criteria approved"}</span>
      </div>
      <p><code>{approval.assessment.releaseRef}</code> / recorded <time dateTime={approval.recordedAt}>{new Date(approval.recordedAt).toLocaleString()}</time></p>
      {approval.skippedOptional.length > 0 && <p className="optional-warning">Acknowledged skipped optional steps: <strong>{approval.skippedOptional.join(", ")}</strong></p>}
      <details className="deployment-provenance"><summary>{preview ? "sample approval" : "approval"} #{approval.id} / frozen decision</summary>
        <AssessmentDetails assessment={approval.assessment} historical />
        <dl className="metadata">
          <div><dt>manifest hash</dt><dd>{approval.assessment.manifestHash}</dd></div>
          <div><dt>source</dt><dd>{approval.assessment.sourcePath}</dd></div>
          <div><dt>revision</dt><dd>{approval.assessment.registrationUpdatedAt}</dd></div>
          {!preview && <div><dt>assessment</dt><dd>{approval.assessment.assessmentHash}</dd></div>}
        </dl>
        {approval.notes && <p className="deployment-note">{approval.notes}</p>}
        <p className="panel-caption">Original criteria, evidence, and skipped warnings are retained. No stage execution or advancement is recorded.</p>
      </details>
    </li>)}
  </ol>;
}
