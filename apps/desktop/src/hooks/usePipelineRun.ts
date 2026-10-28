import { useCallback, useEffect, useRef, useState } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { isRunActive } from "../lib/pipelineRuns";
import { RunOutputBuffer, type OutputSnapshot } from "../lib/runOutput";
import type { PipelineRun, PipesApi, RunLogPage } from "../lib/types";

export const RUN_POLL_INTERVAL = 250;

interface RunState {
  runId: string;
  run: PipelineRun | null;
  output: OutputSnapshot;
  cursor: number;
  loading: boolean;
  error: DisplayError | null;
  actionError: DisplayError | null;
  actionBusy: boolean;
  cancellationRequested: boolean;
}

type Action = { kind: "cancel" } | { kind: "approve"; stageId: string };
interface Controller {
  refresh(): void;
  enqueue(action: Action): void;
}

function initialState(runId: string): RunState {
  return {
    runId, run: null, output: { lines: [], discardedLines: 0, discardedCharacters: 0 }, cursor: 0,
    loading: true, error: null, actionError: null, actionBusy: false, cancellationRequested: false,
  };
}

function validatePage(page: RunLogPage, cursor: number): void {
  if (!Number.isSafeInteger(page.nextCursor) || page.nextCursor < cursor ||
    page.nextCursor !== (page.entries.at(-1)?.sequence ?? cursor) ||
    (page.hasMore && page.nextCursor === cursor) ||
    page.entries.some((entry, index) => !Number.isSafeInteger(entry.sequence) || entry.sequence <= cursor ||
      entry.sequence > page.nextCursor || (index > 0 && entry.sequence <= page.entries[index - 1]!.sequence))) {
    throw { code: "invalid_log_page", message: "The log cursor did not advance safely. Output has been kept; retry from the last confirmed cursor." };
  }
}

export function usePipelineRun(api: PipesApi, runId: string, onUpdate?: (run: PipelineRun) => void) {
  const [state, setState] = useState<RunState>(() => initialState(runId));
  const controller = useRef<Controller | null>(null);
  const updateCallback = useRef(onUpdate);
  updateCallback.current = onUpdate;

  useEffect(() => {
    let current = true;
    let inFlight = false;
    let requested = false;
    let action: Action | null = null;
    let busy = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cursor = 0;
    let run: PipelineRun | null = null;
    const buffer = new RunOutputBuffer();
    setState(initialState(runId));

    const update = (patch: Partial<RunState>) => {
      if (current) setState((previous) => ({ ...previous, ...patch }));
    };
    const accept = (next: PipelineRun) => {
      if (next.id !== runId) throw { code: "run_mismatch", message: "The service returned a different run. Refresh to retry." };
      run = next;
      update({ run: next, loading: false });
      updateCallback.current?.(next);
    };

    async function tick() {
      if (!current || inFlight) return;
      if (!api.available) {
        update({ loading: false, error: { code: "desktop_required", message: "Open the desktop app to inspect real pipeline runs." } });
        return;
      }
      inFlight = true;
      requested = false;
      let retry = false;
      let more = false;
      try {
        const pending = action;
        action = null;
        if (pending) {
          try {
            const next = pending.kind === "cancel"
              ? await api.cancelPipelineRun(runId)
              : await api.approvePipelineStage(runId, pending.stageId, true);
            if (!current) return;
            accept(next);
            update({ actionError: null, ...(pending.kind === "cancel" ? { cancellationRequested: true } : {}) });
          } catch (failure) {
            if (!current) return;
            update({ actionError: displayError(failure) });
            requested = true;
          } finally {
            busy = false;
            update({ actionBusy: false });
          }
        } else {
          const next = await api.getPipelineRun(runId);
          if (!current) return;
          accept(next);
        }

        do {
          const page = await api.pipelineRunLogs(runId, cursor);
          if (!current) return;
          validatePage(page, cursor);
          buffer.append(page.entries);
          cursor = page.nextCursor;
          more = page.hasMore;
          update({ cursor, output: buffer.snapshot(), error: null });
          // User actions take priority over draining a very large persisted log.
        } while (more && !action && current);
        if (run && !isRunActive(run.status) && !more) {
          buffer.finish();
          update({ output: buffer.snapshot() });
        }
      } catch (failure) {
        if (!current) return;
        const error = displayError(failure);
        update({ error, loading: false });
        retry = !["not_found", "run_not_found", "desktop_required"].includes(error.code);
      } finally {
        inFlight = false;
        if (current && (requested || action || more || retry || (run && isRunActive(run.status)))) {
          timer = setTimeout(() => { void tick(); }, requested || action || more ? 0 : retry ? 1000 : RUN_POLL_INTERVAL);
        }
      }
    }

    function wake() {
      requested = true;
      clearTimeout(timer);
      if (!inFlight) void tick();
    }
    const controls: Controller = {
      refresh: wake,
      enqueue: (next) => {
        if (busy || !current || !run || !isRunActive(run.status)) return;
        busy = true;
        action = next;
        update({ actionBusy: true, actionError: null });
        wake();
      },
    };
    controller.current = controls;
    queueMicrotask(() => { if (current) void tick(); });
    return () => {
      current = false;
      clearTimeout(timer);
      if (controller.current === controls) controller.current = null;
    };
  }, [api, runId]);

  const refresh = useCallback(() => controller.current?.refresh(), []);
  const cancel = useCallback(() => controller.current?.enqueue({ kind: "cancel" }), []);
  const approve = useCallback((stageId: string) => controller.current?.enqueue({ kind: "approve", stageId }), []);
  return { ...(state.runId === runId ? state : initialState(runId)), refresh, cancel, approve };
}
