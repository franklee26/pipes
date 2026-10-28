import { useId, useRef, useState, type FormEvent } from "react";
import { displayError, type DisplayError } from "../lib/api";
import type { DeploymentReview } from "../lib/consumerDeployments";
import { noteProblem, releaseReferenceProblem } from "../lib/inputValidation";
import type { ConsumerDeployment, RecordConsumerDeployment } from "../lib/types";
import { ErrorBanner } from "./ErrorBanner";
import { Arrow, Device } from "./Icons";
import { Modal } from "./Modal";
import "./ConsumerDeployments.css";

export function RecordDeploymentDialog({
  review,
  onConfirm,
  onRecorded,
  onClose,
  onInvalidated,
  onReturnFocus,
  initialReleaseRef = "",
  preview = false,
}: {
  review: DeploymentReview;
  onConfirm(request: RecordConsumerDeployment): Promise<ConsumerDeployment>;
  onRecorded(record: ConsumerDeployment): void;
  onClose(): void;
  onInvalidated?(): void;
  onReturnFocus?(): void;
  initialReleaseRef?: string;
  preview?: boolean;
}) {
  const id = useId();
  const [releaseRef, setReleaseRef] = useState(initialReleaseRef);
  const [notes, setNotes] = useState("");
  const [approved, setApproved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [invalidated, setInvalidated] = useState(false);
  const [error, setError] = useState<DisplayError | null>(null);
  const pending = useRef(false);
  const referenceProblem = releaseReferenceProblem(releaseRef);
  const notesProblem = noteProblem(notes);
  const fieldProblem = referenceProblem ?? notesProblem;

  async function record(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending.current) return;
    if (invalidated) {
      setError({ code: "registration_changed", message: "Close this dialog and review the current consumer before recording." });
      return;
    }
    if (!releaseRef.trim() || !approved) {
      setError({ code: "approval_required", message: "Enter a release reference and explicitly confirm the manual deployment." });
      return;
    }
    if (fieldProblem) {
      setError({ code: "invalid_deployment", message: fieldProblem });
      return;
    }
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      const result = await onConfirm({
        appId: review.appId,
        stageId: review.stageId,
        consumerId: review.consumer.id,
        expectedManifestHash: review.manifestHash,
        expectedUpdatedAt: review.updatedAt,
        releaseRef: releaseRef.trim(),
        notes: notes.trim() || null,
        approved: true,
      });
      onRecorded(result);
    } catch (failure) {
      const issue = displayError(failure);
      setError(issue);
      if (["registration_changed", "consumer_not_found", "not_found"].includes(issue.code)) {
        setInvalidated(true);
        setApproved(false);
        onInvalidated?.();
      }
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  return (
    <Modal title={`deployed to / ${review.consumer.name}`} busy={busy} onClose={onClose} onReturnFocus={onReturnFocus}>
      <div className="deployment-review-route"><span className="meta-tag">{review.appName} / {review.stageId}</span><Arrow /><Device /><strong>{review.consumer.name}</strong></div>
      <code className="consumer-target">{review.consumer.target}</code>
      <p className="compact-note">{preview ? "Simulation only: this marker is not saved." : "Approve a manual history marker for a deployment you performed outside Pipes."} No files are copied, no commands run, and the stage remains unchanged.</p>
      <form className="deployment-form" onSubmit={(event) => { void record(event); }}>
        <label htmlFor={`${id}-reference`}>Release reference</label>
        <input
          id={`${id}-reference`}
          value={releaseRef}
          required
          maxLength={512}
          data-autofocus
          autoComplete="off"
          spellCheck={false}
          disabled={busy || invalidated}
          placeholder="v1.5.0, commit SHA, or artifact digest"
          aria-invalid={Boolean(referenceProblem)}
          aria-describedby={`${id}-reference-help${referenceProblem ? ` ${id}-reference-error` : ""}`}
          onChange={(event) => { setReleaseRef(event.currentTarget.value); setApproved(false); }}
        />
        <span className="panel-caption" id={`${id}-reference-help`}>Version, commit, or artifact reference, up to 256 characters. Recorded as supplied; not verified.</span>
        {referenceProblem && <p className="deployment-field-error" id={`${id}-reference-error`} role="status">{referenceProblem}</p>}
        <label htmlFor={`${id}-notes`}>Note <span className="muted">(optional)</span></label>
        <textarea
          id={`${id}-notes`}
          rows={3}
          maxLength={8192}
          value={notes}
          disabled={busy || invalidated}
          aria-invalid={Boolean(notesProblem)}
          aria-describedby={`${id}-notes-help${notesProblem ? ` ${id}-notes-error` : ""}`}
          onChange={(event) => setNotes(event.currentTarget.value)}
        />
        <span className="panel-caption" id={`${id}-notes-help`}>Up to 4096 characters. Notes are retained with the immutable record.</span>
        {notesProblem && <p className="deployment-field-error" id={`${id}-notes-error`} role="status">{notesProblem}</p>}
        <label className="deployment-approval" htmlFor={`${id}-approved`}>
          <input id={`${id}-approved`} type="checkbox" checked={approved} disabled={busy || invalidated} onChange={(event) => setApproved(event.currentTarget.checked)} />
          <span>{preview ? "Simulate my approval: " : "I confirm that "}I manually deployed this reference to {review.consumer.name}.</span>
        </label>
        {error && <ErrorBanner error={error} />}
        {invalidated && <p className="compact-note">The reviewed destination is no longer current. Close this dialog and review it again; nothing was recorded.</p>}
        <details className="disclosure deployment-review">
          <summary>reviewed registration</summary>
          <dl className="metadata">
            <div><dt>app / stage</dt><dd>{review.appId} / {review.stageId}</dd></div>
            <div><dt>consumer</dt><dd>{review.consumer.id}</dd></div>
            <div><dt>SHA-256</dt><dd>{review.manifestHash}</dd></div>
            <div><dt>revision</dt><dd>{review.updatedAt}</dd></div>
            <div><dt>manifest</dt><dd>{review.manifestPath}</dd></div>
            <div><dt>source</dt><dd>{review.sourcePath}</dd></div>
          </dl>
        </details>
        <div className="modal-actions">
          <button type="button" className="button" disabled={busy} onClick={onClose}>cancel</button>
          <button type="submit" className="button primary" disabled={busy || invalidated || !approved || !releaseRef.trim() || Boolean(fieldProblem)}>
            {busy ? "recording..." : preview ? "simulate approval & record" : "approve & record"}<Arrow />
          </button>
        </div>
      </form>
    </Modal>
  );
}
