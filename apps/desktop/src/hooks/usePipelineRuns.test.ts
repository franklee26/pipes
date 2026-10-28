import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { summarizeRun } from "../lib/pipelineRuns";
import type { PipelineRunSummary } from "../lib/types";
import { makeApi, makeRun } from "../test/fixtures";
import { usePipelineRuns } from "./usePipelineRuns";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

async function advance(ms = 0) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

describe("persistent run history state", () => {
  it("loads saved runs independently of current registrations and preserves history on reload", async () => {
    const run = makeRun({ status: "passed" });
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    const first = renderHook(() => usePipelineRuns(api));
    await advance();
    expect(first.result.current.runs).toEqual([summarizeRun(run)]);
    first.unmount();
    const second = renderHook(() => usePipelineRuns(api));
    await advance();
    expect(second.result.current.runs).toEqual([summarizeRun(run)]);
    expect(api.listApps).not.toHaveBeenCalled();
    expect(api.startPipelineRun).not.toHaveBeenCalled();
  });

  it("keeps a confirmed start when an older list request resolves late", async () => {
    const run = makeRun({ status: "passed" });
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    let resolve!: (value: PipelineRunSummary[]) => void;
    api.listPipelineRuns.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    const { result } = renderHook(() => usePipelineRuns(api));
    await advance();
    act(() => result.current.put(run));
    await act(async () => resolve([]));
    expect(result.current.runs).toEqual([summarizeRun(run)]);
    await advance();
    expect(result.current.runs).toEqual([summarizeRun(run)]);
    expect(api.listPipelineRuns).toHaveBeenCalledTimes(2);
  });

  it("serializes refreshes, refreshes active summaries and stops after unmount", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    let resolve!: (value: PipelineRunSummary[]) => void;
    api.listPipelineRuns.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    const { result, unmount } = renderHook(() => usePipelineRuns(api));
    await advance();
    act(() => { result.current.refresh(); result.current.refresh(); });
    await advance(5000);
    expect(api.listPipelineRuns).toHaveBeenCalledOnce();
    await act(async () => resolve([summarizeRun(run)]));
    await advance();
    expect(api.listPipelineRuns).toHaveBeenCalledTimes(2);
    await advance(2000);
    expect(api.listPipelineRuns).toHaveBeenCalledTimes(3);
    unmount();
    await advance(10000);
    expect(api.listPipelineRuns).toHaveBeenCalledTimes(3);
  });

  it("does not call native run history methods in an unavailable browser", async () => {
    const api = makeApi([], false);
    const { result } = renderHook(() => usePipelineRuns(api));
    await advance();
    expect(result.current.loading).toBe(false);
    expect(result.current.runs).toBeNull();
    expect(api.listPipelineRuns).not.toHaveBeenCalled();
  });

  it("fetches app-scoped history only when enabled, instead of filtering the global 100-run page", async () => {
    const run = makeRun({ status: "passed" });
    const other = makeRun({ id: "other", appId: "other", status: "passed" });
    const api = makeApi([], true, run.manifest, [], { runs: [other, run] });
    const { result, rerender } = renderHook(({ enabled }) => usePipelineRuns(api, "atlas", enabled), { initialProps: { enabled: false } });
    await advance();
    expect(api.listPipelineRuns).not.toHaveBeenCalled();
    rerender({ enabled: true });
    await advance();
    expect(api.listPipelineRuns).toHaveBeenCalledExactlyOnceWith("atlas");
    expect(result.current.runs?.map((item) => item.id)).toEqual([run.id]);
    rerender({ enabled: false });
    act(() => result.current.refresh());
    await advance(5000);
    expect(api.listPipelineRuns).toHaveBeenCalledOnce();
  });

  it("ignores a late response from a previous app scope", async () => {
    const run = makeRun({ status: "passed" });
    const other = makeRun({ id: "other", appId: "other", status: "passed" });
    const api = makeApi([], true, run.manifest, [], { runs: [other, run] });
    let resolve!: (runs: PipelineRunSummary[]) => void;
    api.listPipelineRuns.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    const { result, rerender } = renderHook(({ appId }) => usePipelineRuns(api, appId), { initialProps: { appId: "atlas" } });
    await advance();
    rerender({ appId: "other" });
    await advance();
    expect(result.current.runs?.map((item) => item.id)).toEqual([other.id]);
    await act(async () => resolve([summarizeRun(run)]));
    expect(result.current.runs?.map((item) => item.id)).toEqual([other.id]);
  });

  it("retains the latest 100 summaries when accepting a new run", async () => {
    const runs = Array.from({ length: 100 }, (_, index) => makeRun({ id: `old-${index}`, status: "passed" }));
    const api = makeApi([], true, runs[0]!.manifest, [], { runs });
    const { result } = renderHook(() => usePipelineRuns(api));
    await advance();
    const latest = makeRun({ id: "latest", status: "passed", startedAt: "2026-10-28T00:00:00Z" });
    act(() => result.current.put(latest));
    expect(result.current.runs).toHaveLength(100);
    expect(result.current.runs?.[0]?.id).toBe(latest.id);
  });
});
