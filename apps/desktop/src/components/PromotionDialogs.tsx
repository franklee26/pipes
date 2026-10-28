import { useId, useRef, useState, type FormEvent } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { noteProblem, parseTestCount, releaseReferenceProblem } from "../lib/inputValidation";
import { promotionContext, promotionReady, type PromotionReview } from "../lib/promotion";
import type { ApprovePromotion, HarnessResult, PromotionApproval, PromotionAssessment, RecordHarnessResult } from "../lib/types";
import { AssessmentDetails } from "./PromotionCriteria";
import { ErrorBanner } from "./ErrorBanner";
import { Modal } from "./Modal";
import { Arrow } from "./Icons";
import "./ConsumerDeployments.css";

const invalidReviewCodes = ["registration_changed", "assessment_changed", "not_found", "stage_not_found", "check_not_found"];

export function ReportHarnessDialog({
  review, checkId, initialReleaseRef = "", onConfirm, onRecorded, onClose, onInvalidated, onReturnFocus, preview = false,
}: {
  review: PromotionReview;
  checkId: string;
  initialReleaseRef?: string;
  onConfirm(request: RecordHarnessResult): Promise<HarnessResult>;
  onRecorded(result: HarnessResult): void;
  onClose(): void;
  onInvalidated?(): void;
  onReturnFocus?(): void;
  preview?: boolean;
}) {
  const id = useId();
  const [reference, setReference] = useState(initialReleaseRef);
  const [total, setTotal] = useState("");
  const [passed, setPassed] = useState("");
  const [notes, setNotes] = useState("");
  const [busy, setBusy] = useState(false);
  const [invalidated, setInvalidated] = useState(false);
  const [error, setError] = useState<DisplayError | null>(null);
  const pending = useRef(false);
  const totalCount = parseTestCount(total);
  const passedCount = parseTestCount(passed);
  const problem = releaseReferenceProblem(reference) ?? noteProblem(notes)
    ?? (total && totalCount === null ? "Reported total must be a whole number from 0 to 4294967295." : null)
    ?? (passed && passedCount === null ? "Passed count must be a whole number from 0 to 4294967295." : null)
    ?? (totalCount !== null && passedCount !== null && passedCount > totalCount ? "Passed tests cannot exceed the reported total." : null);
  async function record(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending.current) return;
    if (invalidated || !reference.trim() || totalCount === null || passedCount === null || problem) {
      setError({ code: "invalid_harness_result", message: invalidated ? "Close and review the current harness configuration." : problem ?? "Enter the reference and both observed counts." });
      return;
    }
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      const result = await onConfirm({ ...promotionContext(review, reference), checkId, total: totalCount, passed: passedCount, notes: notes.trim() || null });
      onRecorded(result);
    } catch (failure) {
      const issue = displayError(failure);
      setError(issue);
      if (invalidReviewCodes.includes(issue.code)) { setInvalidated(true); onInvalidated?.(); }
    } finally { pending.current = false; setBusy(false); }
  }
  return <Modal title={`report harness / ${checkId}`} busy={busy} onClose={onClose} onReturnFocus={onReturnFocus}>
    <p className="compact-note">{preview ? "Simulation only. " : ""}Record externally produced test counts for {review.appName} / {review.stage.id}. Reporting does not execute commands or independently verify these counts.</p>
    <form className="deployment-form" onSubmit={(event) => { void record(event); }}>
      <label htmlFor={`${id}-reference`}>Release reference</label>
      <input id={`${id}-reference`} data-autofocus value={reference} required maxLength={512} disabled={busy || invalidated} onChange={(event) => setReference(event.currentTarget.value)} />
      <div className="harness-count-fields">
        <label htmlFor={`${id}-total`}>Tests reported<input id={`${id}-total`} type="number" min={0} max={4294967295} step={1} required value={total} disabled={busy || invalidated} onChange={(event) => setTotal(event.currentTarget.value)} /></label>
        <label htmlFor={`${id}-passed`}>Tests passed<input id={`${id}-passed`} type="number" min={0} max={4294967295} step={1} required value={passed} disabled={busy || invalidated} onChange={(event) => setPassed(event.currentTarget.value)} /></label>
      </div>
      <p className="panel-caption">Enter actual totals, including failed or skipped cases. A different suite size is recorded but cannot satisfy an exact-n policy. Zero tests is not a pass.</p>
      <label htmlFor={`${id}-note`}>Note (optional)</label>
      <textarea id={`${id}-note`} rows={3} maxLength={8192} value={notes} disabled={busy || invalidated} onChange={(event) => setNotes(event.currentTarget.value)} />
      {problem && <p className="deployment-field-error" role="status">{problem}</p>}
      {error && <ErrorBanner error={error} />}
      {invalidated && <p className="compact-note">The reviewed configuration changed. Close this dialog and review it again.</p>}
      <div className="modal-actions"><button type="button" className="button" onClick={onClose} disabled={busy}>cancel</button><button type="submit" className="button primary" disabled={busy || invalidated || !reference.trim() || totalCount === null || passedCount === null || Boolean(problem)}>
        {busy ? "recording..." : preview ? "save sample results" : "record results"}<Arrow />
      </button></div>
    </form>
  </Modal>;
}

export function ApprovePromotionDialog({
  assessment, onConfirm, onRecorded, onClose, onInvalidated, onReturnFocus, preview = false,
}: {
  assessment: PromotionAssessment;
  onConfirm(request: ApprovePromotion): Promise<PromotionApproval>;
  onRecorded(approval: PromotionApproval): void;
  onClose(): void;
  onInvalidated?(): void;
  onReturnFocus?(): void;
  preview?: boolean;
}) {
  const id = useId();
  const [approved, setApproved] = useState(false);
  const [acknowledged, setAcknowledged] = useState(false);
  const [notes, setNotes] = useState("");
  const [busy, setBusy] = useState(false);
  const [invalidated, setInvalidated] = useState(false);
  const [error, setError] = useState<DisplayError | null>(null);
  const pending = useRef(false);
  const warnings = assessment.criteria.filter((criterion) => criterion.status === "warning");
  const problem = noteProblem(notes);
  async function approve(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending.current) return;
    if (invalidated || !promotionReady(assessment) || !approved || (warnings.length > 0 && !acknowledged) || problem) {
      setError({ code: "approval_required", message: problem ?? "Review a ready assessment and acknowledge every skipped optional step before approving." });
      return;
    }
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      const result = await onConfirm({
        appId: assessment.appId, stageId: assessment.stageId, releaseRef: assessment.releaseRef,
        expectedManifestHash: assessment.manifestHash, expectedUpdatedAt: assessment.registrationUpdatedAt,
        expectedAssessmentHash: assessment.assessmentHash, approved: true,
        acknowledgeOptional: acknowledged, notes: notes.trim() || null,
      });
      onRecorded(result);
    } catch (failure) {
      const issue = displayError(failure);
      setError(issue);
      if (invalidReviewCodes.includes(issue.code) || issue.code === "promotion_blocked" || issue.code === "promotion_not_configured") {
        setInvalidated(true); setApproved(false); setAcknowledged(false); onInvalidated?.();
      }
    } finally { pending.current = false; setBusy(false); }
  }
  return <Modal title={`approve promotion / ${assessment.stageId}`} busy={busy} onClose={onClose} onReturnFocus={onReturnFocus} wide>
    <p className="compact-note">{preview ? "Simulation only. " : ""}Approve the declared promotion criteria for <strong>{assessment.appName} / {assessment.releaseRef}</strong>. {preview
      ? "This records a sample decision only; no commands run and no stage advances."
      : "This records a policy decision. An already approved pipeline run waiting on this candidate and registration may continue when the runner rechecks it. This does not start a new run or grant approval.local."}</p>
    <AssessmentDetails assessment={assessment} />
    <form className="deployment-form" onSubmit={(event) => { void approve(event); }}>
      {warnings.length > 0 && <label className="deployment-approval" htmlFor={`${id}-skips`}>
        <input id={`${id}-skips`} type="checkbox" checked={acknowledged} disabled={busy || invalidated} onChange={(event) => setAcknowledged(event.currentTarget.checked)} />
        <span>I acknowledge that these optional steps are skipped: <strong>{warnings.map((warning) => warning.criterion.id).join(", ")}</strong>.</span>
      </label>}
      <label className="deployment-approval" htmlFor={`${id}-approved`}>
        <input id={`${id}-approved`} type="checkbox" checked={approved} disabled={busy || invalidated} onChange={(event) => setApproved(event.currentTarget.checked)} />
        <span>I approve this promotion-criteria decision, not a claim that the pipeline has executed.</span>
      </label>
      <label htmlFor={`${id}-note`}>Approval note (optional)</label>
      <textarea id={`${id}-note`} rows={2} maxLength={8192} value={notes} disabled={busy || invalidated} onChange={(event) => setNotes(event.currentTarget.value)} />
      {problem && <p className="deployment-field-error" role="status">{problem}</p>}
      {error && <ErrorBanner error={error} />}
      {invalidated && <p className="compact-note">Evidence or configuration changed. Close and assess readiness again; this review cannot be reused.</p>}
      <div className="modal-actions"><button type="button" className="button" onClick={onClose} disabled={busy}>cancel</button><button type="submit" className="button primary" disabled={busy || invalidated || !promotionReady(assessment) || !approved || (warnings.length > 0 && !acknowledged) || Boolean(problem)}>
        {busy ? "recording..." : preview ? "simulate promotion approval" : "approve & record decision"}<Arrow />
      </button></div>
    </form>
  </Modal>;
}
