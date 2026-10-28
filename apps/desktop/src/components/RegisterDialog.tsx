import { useState, type FormEvent } from "react";
import { displayError, type DisplayError } from "../lib/api";
import type { ManifestPreview, PipesApi, RegisteredApp } from "../lib/types";
import { ErrorBanner } from "./ErrorBanner";
import { RunBudget } from "./CheckPlan";
import { ManifestPlan } from "./ManifestPlan";
import { Modal } from "./Modal";

interface RegisterDialogProps {
  api: PipesApi;
  apps: RegisteredApp[];
  initialPath?: string;
  onClose(): void;
  onRegistered(app: RegisteredApp): void;
  onRefresh(): void;
}

export function RegisterDialog({
  api,
  apps,
  initialPath = "",
  onClose,
  onRegistered,
  onRefresh,
}: RegisterDialogProps) {
  const [path, setPath] = useState(initialPath);
  const [preview, setPreview] = useState<ManifestPreview | null>(null);
  const [error, setError] = useState<DisplayError | null>(null);
  const [pending, setPending] = useState<"picker" | "preview" | "register" | null>(null);
  const [replace, setReplace] = useState(false);
  const existing = apps.find((app) => app.manifest.appId === preview?.appId);
  const needsReplace = Boolean(existing && preview && (
    existing.manifest.manifestHash !== preview.manifestHash ||
    existing.manifest.manifestPath !== preview.manifestPath ||
    existing.manifest.sourcePath !== preview.sourcePath
  ));
  const busy = pending !== null;

  async function readPreview(candidate: string) {
    setPreview(null);
    setReplace(false);
    setError(null);
    setPending("preview");
    const result = await api.previewManifest(candidate);
    setPreview(result);
  }

  async function browse() {
    setPending("picker");
    setError(null);
    try {
      const selected = await api.chooseManifest();
      if (selected !== null) {
        setPath(selected);
        await readPreview(selected);
      }
    } catch (failure) {
      setError(displayError(failure));
    } finally {
      setPending(null);
    }
  }

  async function review(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    try {
      await readPreview(path);
    } catch (failure) {
      setError(displayError(failure));
    } finally {
      setPending(null);
    }
  }

  async function register() {
    if (preview === null) {
      setError({ code: "preview_required", message: "Review a manifest before registering it." });
      return;
    }
    setPending("register");
    setError(null);
    try {
      const app = await api.registerManifest(
        preview.manifestPath,
        preview.manifestHash,
        needsReplace && replace,
      );
      onRegistered(app);
    } catch (failure) {
      const issue = displayError(failure);
      setError(issue);
      if (issue.code === "manifest_changed") {
        setPreview(null);
        setReplace(false);
      }
      if (issue.code === "already_registered") onRefresh();
    } finally {
      setPending(null);
    }
  }

  return (
    <Modal title="register manifest" onClose={onClose} busy={busy} wide>
      <form onSubmit={(event) => { void review(event); }} className="manifest-form">
        <label htmlFor="manifest-path">Manifest path</label>
        <div className="path-field">
          <input
            id="manifest-path"
            value={path}
            data-autofocus
            disabled={busy}
            placeholder="/path/to/pipes.yaml"
            spellCheck={false}
            onChange={(event) => {
              setPath(event.currentTarget.value);
              setPreview(null);
              setError(null);
              setReplace(false);
            }}
          />
          <button type="button" className="button" onClick={() => { void browse(); }} disabled={busy}>
            browse
          </button>
          <button type="submit" className="button" disabled={busy || path.trim() === ""}>
            {pending === "preview" ? "reading..." : "review"}
          </button>
        </div>
      </form>
      {error && <ErrorBanner error={error} />}
      {preview && (
        <div className="registration-preview">
          <header className="preview-heading">
            <h3>{preview.name}</h3>
            <span className="status-label">valid manifest</span>
          </header>
          <dl className="metadata">
            <div><dt>app</dt><dd>{preview.appId}</dd></div>
            <div><dt>source</dt><dd>{preview.sourcePath}</dd></div>
            <div><dt>version</dt><dd>semver / {preview.versionFrom}</dd></div>
            <div><dt>parallel</dt><dd><RunBudget value={preview.execution.maxParallel} /></dd></div>
          </dl>
          <ManifestPlan key={preview.manifestHash} manifest={preview} />
          <details className="disclosure">
            <summary>manifest snapshot / {preview.manifestHash.slice(0, 12)}</summary>
            <pre className="yaml">{preview.manifestYaml}</pre>
          </details>
          {needsReplace && (
            <label className="replace-confirmation">
              <input
                type="checkbox"
                checked={replace}
                disabled={busy}
                onChange={(event) => setReplace(event.currentTarget.checked)}
              />
              Replace the existing registered snapshot.
            </label>
          )}
          {existing && !needsReplace && <p className="compact-note">This snapshot is already registered.</p>}
          <footer className="modal-actions">
            <span className="muted">Registration runs no commands.</span>
            <button
              type="button"
              className="button primary"
              disabled={busy || (needsReplace && !replace)}
              onClick={() => { void register(); }}
            >
              {pending === "register" ? "saving..." : needsReplace ? "replace registration" : "register"}
            </button>
          </footer>
        </div>
      )}
      {!preview && <p className="compact-note">Review first. Nothing executes.</p>}
    </Modal>
  );
}
