import { useCallback, useEffect, useRef, useState } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { isRunActive, PIPELINE_RUN_HISTORY_LIMIT, summarizeRun } from "../lib/pipelineRuns";
import type { PipelineRun, PipelineRunSummary, PipesApi } from "../lib/types";

export function usePipelineRuns(api: PipesApi, appId?: string, enabled = true) {
  const [runs, setRuns] = useState<PipelineRunSummary[] | null>(null);
  const [scope, setScope] = useState(appId);
  const [loading, setLoading] = useState(api.available && enabled);
  const [error, setError] = useState<DisplayError | null>(null);
  const load = useRef<(() => void) | null>(null);
  const version = useRef(0);

  useEffect(() => {
    let current = true;
    let inFlight = false;
    let requested = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setRuns(null);
    setScope(appId);
    setError(null);
    setLoading(api.available && enabled);
    if (!enabled) return;
    async function tick() {
      if (!current || inFlight || !api.available) return;
      requested = false;
      inFlight = true;
      const startedVersion = version.current;
      let active = false;
      try {
        const next = await api.listPipelineRuns(appId);
        if (!current) return;
        if (startedVersion === version.current) setRuns(next);
        else requested = true;
        setError(null);
        active = next.some((run) => isRunActive(run.status));
      } catch (failure) {
        if (current) setError(displayError(failure));
      } finally {
        inFlight = false;
        if (current) {
          setLoading(false);
          if (requested || active) timer = setTimeout(() => { void tick(); }, requested ? 0 : 2000);
        }
      }
    }
    function refresh() {
      if (!current || !api.available) return;
      clearTimeout(timer);
      requested = true;
      if (!inFlight) {
        setLoading(true);
        queueMicrotask(() => { if (current) void tick(); });
      }
    }
    load.current = refresh;
    window.addEventListener("focus", refresh);
    queueMicrotask(() => { if (current) void tick(); });
    return () => {
      current = false;
      clearTimeout(timer);
      window.removeEventListener("focus", refresh);
      if (load.current === refresh) load.current = null;
    };
  }, [api, appId, enabled]);

  const refresh = useCallback(() => load.current?.(), []);
  const put = useCallback((run: PipelineRun) => {
    if (!enabled || (appId !== undefined && run.appId !== appId)) return;
    version.current += 1;
    const summary = summarizeRun(run);
    setRuns((previous) => [summary, ...(previous ?? []).filter((item) => item.id !== run.id)]
      .sort((left, right) => right.startedAt.localeCompare(left.startedAt) || right.id.localeCompare(left.id))
      .slice(0, PIPELINE_RUN_HISTORY_LIMIT));
  }, [appId, enabled]);
  return {
    runs: enabled && scope === appId ? runs : null,
    loading: scope === appId ? loading : api.available && enabled,
    error: enabled && scope === appId ? error : null,
    refresh, put,
  };
}
