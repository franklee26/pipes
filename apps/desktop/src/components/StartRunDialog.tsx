import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { releaseReferenceProblem } from "../lib/inputValidation";
import type { PipelineRun, RegisteredApp, StartPipelineRun } from "../lib/types";
import { CheckPlan, RunBudget } from "./CheckPlan";
import { CommandList } from "./CommandList";
import { ErrorBanner } from "./ErrorBanner";
import { Play } from "./Icons";
import { Modal } from "./Modal";
import "./PipelineRuns.css";

export function StartRunDialog({
  review,
  onConfirm,
  onStarted,
  onClose,
  onInvalidated,
  onReturnFocus,
}: {
  review: RegisteredApp;
  onConfirm(request: StartPipelineRun): Promise<PipelineRun>;
  onStarted(run: PipelineRun): void;
  onClose(): void;
  onInvalidated(): void;
  onReturnFocus?(): void;
}) {
  const [reviewed] = useState(() => structuredClone(review));
  const manifest = reviewed.manifest;
  const id = useId();
  const [releaseRef, setReleaseRef] = useState("");
  const [approved, setApproved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [invalidated, setInvalidated] = useState(false);
  const [error, setError] = useState<DisplayError | null>(null);
  const pending = useRef(false);
  const current = useRef(true);
  const problem = releaseReferenceProblem(releaseRef);
  const commands = manifest.prepare.length + manifest.stages.reduce((count, stage) => count + stage.checks.length, 0);

  useEffect(() => {
    current.current = true;
    return () => { current.current = false; };
  }, []);

  async function start(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending.current || invalidated || !approved || !releaseRef.trim() || problem) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    try {
      const run = await onConfirm({
        appId: manifest.appId,
        expectedManifestHash: manifest.manifestHash,
        expectedUpdatedAt: reviewed.updatedAt,
        releaseRef: releaseRef.trim(),
        approved: true,
      });
      if (current.current) onStarted(run);
    } catch (failure) {
      if (!current.current) return;
      const issue = displayError(failure);
      setError(issue);
      setApproved(false);
      if (["registration_changed", "manifest_changed", "not_found"].includes(issue.code)) setInvalidated(true);
      onInvalidated();
    } finally {
      pending.current = false;
      if (current.current) setBusy(false);
    }
  }

  return (
    <Modal title={`review execution / ${manifest.name}`} wide busy={busy} onClose={onClose} onReturnFocus={onReturnFocus}>
      <div className="execution-warning">
        <strong>Real commands. Current source files.</strong>
        <p>This runs {commands} arbitrary manifest {commands === 1 ? "command" : "commands"} on this machine, with your user permissions. Commands can modify files and access the network. Review the arguments, environment, and result contracts before approving.</p>
      </div>
      <dl className="metadata run-review-metadata">
        <div><dt>mode</dt><dd>worktree / requires explicit <code>source.snapshot: working-tree</code></dd></div>
        <div><dt>source / default cwd</dt><dd>{manifest.sourcePath}</dd></div>
        <div><dt>manifest</dt><dd>{manifest.manifestPath}</dd></div>
        <div><dt>SHA-256</dt><dd>{manifest.manifestHash}</dd></div>
        <div><dt>registration revision</dt><dd>{reviewed.updatedAt}</dd></div>
        <div><dt>parallel commands</dt><dd><RunBudget value={manifest.execution.maxParallel} /></dd></div>
        <div><dt>default command timeout</dt><dd>{manifest.execution.timeout ?? "none / until exit or cancellation"}</dd></div>
      </dl>
      <p className="compact-note">The registered YAML is frozen for the run; source contents are not pinned. Execution uses the current working tree. Clean worktrees are required by default; allowed local changes are part of the run. Relative command cwd paths resolve from the source directory.</p>
      <p className="compact-note">Git-commit snapshots, including the default when omitted, are unsupported for execution. To opt in, edit the YAML to set <code>source.snapshot: working-tree</code>, review it, and re-register the manifest. This dialog never changes source policy.</p>
      <p className="compact-note">Keychain secret references are not supported by the runner and are rejected before execution. Only one active run may use the same source directory within this registry.</p>
      <p className="compact-note">All check leaves are required. Prepare commands run sequentially and must all pass before any stage begins. A failed branch does not cancel other independent branches; those may finish normally.</p>
      <details className="run-review-commands" open>
        <summary>review {commands} {commands === 1 ? "command" : "commands"} / {manifest.stages.length} {manifest.stages.length === 1 ? "stage" : "stages"}</summary>
        {manifest.prepare.length > 0 && <section><h3 className="eyebrow">prepare / before stages</h3><CommandList commands={manifest.prepare} sourcePath={manifest.sourcePath} /></section>}
        {manifest.stages.map((stage) => (
          <section className="run-review-stage" key={stage.id} aria-label={`Review ${stage.id} execution`}>
            <h3>{stage.id} <span className="muted">/ {stage.after.length ? `after all: ${stage.after.join(", ")}` : "entry stage"}</span></h3>
            {stage.requiresApproval && <p className="approval-note">approval.local: pauses BEFORE this stage’s commands. A separate, per-run approval is required.</p>}
            <CheckPlan steps={stage.checkPlan} parallelism={manifest.execution.maxParallel} sourcePath={manifest.sourcePath} />
            {stage.promotionCriteria.length > 0 && <p className="compact-note">{stage.promotionCriteria.length} custom promotion criteria remain separate policy gates. Passing commands does not bypass or approve those policies.</p>}
          </section>
        ))}
      </details>
      <details className="disclosure"><summary>frozen YAML to execute</summary><pre className="yaml">{manifest.manifestYaml}</pre></details>
      <p className="compact-note">Command outcomes use exit codes or a declared JSON result file; Pipes does not infer test counts. This is noninteractive execution, not a shell or PTY. Output appears when the program flushes it. Runs are attached to the app/CLI process; shutting it down interrupts active execution.</p>
      <form className="run-start-form" onSubmit={(event) => { void start(event); }}>
        <label htmlFor={`${id}-release`}>Release reference</label>
        <input
          id={`${id}-release`}
          required
          data-autofocus
          autoComplete="off"
          spellCheck={false}
          maxLength={512}
          placeholder="v1.5.0 or a candidate identifier"
          value={releaseRef}
          disabled={busy || invalidated}
          aria-invalid={Boolean(problem)}
          aria-describedby={`${id}-help${problem ? ` ${id}-error` : ""}`}
          onChange={(event) => { setReleaseRef(event.currentTarget.value); setApproved(false); }}
        />
        <p id={`${id}-help`} className="panel-caption">Required label, up to 256 characters. This does not select a commit, verify a release, or create an artifact.</p>
        {problem && <p id={`${id}-error`} className="run-field-error" role="status">{problem}</p>}
        <label className="execution-approval" htmlFor={`${id}-approved`}>
          <input id={`${id}-approved`} type="checkbox" checked={approved} disabled={busy || invalidated} onChange={(event) => setApproved(event.currentTarget.checked)} />
          <span>I approve running these commands against the current source files.</span>
        </label>
        {error && <ErrorBanner error={error} />}
        {invalidated ? <p className="compact-note">The reviewed configuration is no longer current. Close this dialog and review the refreshed registration before starting again.</p> :
          error && <p className="compact-note">If the connection was lost, check run history before retrying; the service may already have accepted the request.</p>}
        <div className="modal-actions">
          <button type="button" className="button" disabled={busy} onClick={onClose}>cancel</button>
          <button type="submit" className="button primary" disabled={busy || invalidated || !approved || !releaseRef.trim() || Boolean(problem)}>
            <Play />{busy ? "starting..." : "approve & start run"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
