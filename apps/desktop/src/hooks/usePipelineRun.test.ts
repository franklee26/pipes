import { act, renderHook } from "@testing-library/react";
import { createElement, StrictMode, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeApi, makeLog, makeRun } from "../test/fixtures";
import type { PipelineRun, RunLogPage } from "../lib/types";
import { RUN_POLL_INTERVAL, usePipelineRun } from "./usePipelineRun";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

async function advance(ms = 0) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("selected pipeline run polling", () => {
  it("shows partial output before completion and resumes the cursor without losing fragments", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    api.getPipelineRun.mockResolvedValueOnce(run).mockResolvedValueOnce(run).mockResolvedValue({ ...run, status: "passed", finishedAt: "2026-10-27T22:01:00Z" });
    api.pipelineRunLogs.mockResolvedValueOnce({ entries: [makeLog("hel")], nextCursor: 1, hasMore: false })
      .mockResolvedValueOnce({ entries: [makeLog("lo", 2)], nextCursor: 2, hasMore: false })
      .mockResolvedValueOnce({ entries: [makeLog("!\n", 3)], nextCursor: 3, hasMore: false });
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    expect(result.current.run?.status).toBe("running");
    expect(result.current.output.lines[0]?.text).toBe("hel");
    await advance(RUN_POLL_INTERVAL);
    expect(result.current.output.lines[0]?.text).toBe("hello");
    await advance(RUN_POLL_INTERVAL);
    expect(result.current.run?.status).toBe("passed");
    expect(result.current.output.lines[0]).toMatchObject({ text: "hello!", complete: true });
    expect(api.pipelineRunLogs.mock.calls).toEqual([[run.id, 0], [run.id, 1], [run.id, 2]]);
    await advance(2000);
    expect(api.getPipelineRun).toHaveBeenCalledTimes(3);
  });

  it("drains every persisted page promptly, including after the run has finished", async () => {
    const run = makeRun({ status: "failed", finishedAt: "2026-10-27T22:01:00Z", message: "Exit code 1." });
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    api.pipelineRunLogs.mockResolvedValueOnce({ entries: [makeLog("first\n", 10)], nextCursor: 10, hasMore: true })
      .mockResolvedValueOnce({ entries: [makeLog("last", 15)], nextCursor: 15, hasMore: false });
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    expect(api.getPipelineRun).toHaveBeenCalledOnce();
    expect(api.pipelineRunLogs.mock.calls).toEqual([[run.id, 0], [run.id, 10]]);
    expect(result.current.output.lines.map((line) => line.text)).toEqual(["first", "last"]);
    expect(result.current.run?.message).toBe("Exit code 1.");
  });

  it("does not overlap slow status and log requests or periodic refreshes", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    const status = deferred<PipelineRun>();
    const logs = deferred<RunLogPage>();
    api.getPipelineRun.mockReturnValueOnce(status.promise);
    api.pipelineRunLogs.mockReturnValueOnce(logs.promise);
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    act(() => { result.current.refresh(); result.current.refresh(); });
    await advance(1500);
    expect(api.getPipelineRun).toHaveBeenCalledOnce();
    expect(api.pipelineRunLogs).not.toHaveBeenCalled();
    await act(async () => status.resolve(run));
    await advance(1500);
    expect(api.getPipelineRun).toHaveBeenCalledOnce();
    expect(api.pipelineRunLogs).toHaveBeenCalledOnce();
    await act(async () => logs.resolve({ entries: [], nextCursor: 0, hasMore: false }));
    await advance();
    expect(api.getPipelineRun).toHaveBeenCalledTimes(2);
  });

  it("does not duplicate initial requests under StrictMode effect replay", async () => {
    const run = makeRun({ status: "passed" });
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    renderHook(() => usePipelineRun(api, run.id), {
      wrapper: ({ children }: { children: ReactNode }) => createElement(StrictMode, null, children),
    });
    await advance();
    expect(api.getPipelineRun).toHaveBeenCalledOnce();
    expect(api.pipelineRunLogs).toHaveBeenCalledOnce();
  });

  it("retries transient errors from the confirmed cursor while retaining visible output", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    api.pipelineRunLogs.mockResolvedValueOnce({ entries: [makeLog("first")], nextCursor: 1, hasMore: false })
      .mockRejectedValueOnce({ code: "storage", message: "Temporarily unavailable." })
      .mockResolvedValueOnce({ entries: [makeLog(" second", 2)], nextCursor: 2, hasMore: false });
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    await advance(RUN_POLL_INTERVAL);
    expect(result.current.error?.code).toBe("storage");
    expect(result.current.output.lines[0]?.text).toBe("first");
    await advance(1000);
    expect(api.pipelineRunLogs.mock.calls).toEqual([[run.id, 0], [run.id, 1], [run.id, 1]]);
    expect(result.current.output.lines[0]?.text).toBe("first second");
    expect(result.current.error).toBeNull();
  });

  it("rejects a nonadvancing page instead of looping or dropping output", async () => {
    const run = makeRun({ status: "passed" });
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    api.pipelineRunLogs.mockResolvedValue({ entries: [], nextCursor: 0, hasMore: true });
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    expect(result.current.error?.code).toBe("invalid_log_page");
    expect(result.current.cursor).toBe(0);
    expect(api.pipelineRunLogs).toHaveBeenCalledOnce();
  });

  it("serializes cancellation behind the current request and drains final output", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    const logs = deferred<RunLogPage>();
    api.pipelineRunLogs.mockReturnValueOnce(logs.promise)
      .mockResolvedValueOnce({ entries: [makeLog("stopped\n", 2)], nextCursor: 2, hasMore: false });
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    act(() => { result.current.cancel(); result.current.cancel(); });
    expect(result.current.actionBusy).toBe(true);
    expect(api.cancelPipelineRun).not.toHaveBeenCalled();
    await act(async () => logs.resolve({ entries: [makeLog("stopping\n")], nextCursor: 1, hasMore: true }));
    await advance();
    expect(api.cancelPipelineRun).toHaveBeenCalledExactlyOnceWith(run.id);
    expect(result.current.run?.status).toBe("cancelled");
    expect(result.current.output.lines.map((line) => line.text)).toEqual(["stopping", "stopped"]);
    expect(result.current.actionBusy).toBe(false);
  });

  it("keeps polling an accepted cancellation until cleanup becomes durably terminal", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    api.cancelPipelineRun.mockResolvedValue({ ...run, message: "Cancellation requested; stopping running commands." });
    api.getPipelineRun.mockResolvedValueOnce(run).mockResolvedValue({ ...run, status: "cancelled", finishedAt: "2026-10-27T22:01:00Z" });
    api.pipelineRunLogs.mockResolvedValueOnce({ entries: [makeLog("running\n")], nextCursor: 1, hasMore: false })
      .mockResolvedValueOnce({ entries: [makeLog("stopping\n", 2)], nextCursor: 2, hasMore: false })
      .mockResolvedValueOnce({ entries: [makeLog("cleanup\n", 3)], nextCursor: 3, hasMore: true })
      .mockResolvedValueOnce({ entries: [makeLog("finished\n", 4)], nextCursor: 4, hasMore: false });
    const { result } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    act(() => result.current.cancel());
    await advance();
    expect(result.current.cancellationRequested).toBe(true);
    expect(result.current.run?.status).toBe("running");
    expect(result.current.cursor).toBe(2);
    await advance(RUN_POLL_INTERVAL);
    expect(result.current.run?.status).toBe("cancelled");
    expect(result.current.output.lines.map((line) => line.text)).toEqual(["running", "stopping", "cleanup", "finished"]);
    expect(result.current.cursor).toBe(4);
    await advance(1000);
    expect(api.getPipelineRun).toHaveBeenCalledTimes(2);
    expect(api.cancelPipelineRun).toHaveBeenCalledOnce();
  });

  it("ignores stale results after switching runs and does not poll after unmount", async () => {
    const first = makeRun({ id: "first", status: "passed" });
    const second = makeRun({ id: "second", status: "running" });
    const api = makeApi([], true, first.manifest, [], { runs: [first, second], logs: { second: [makeLog("second only")] } });
    const old = deferred<PipelineRun>();
    const update = vi.fn();
    api.getPipelineRun.mockImplementation((id) => id === first.id ? old.promise : Promise.resolve(second));
    const { result, rerender, unmount } = renderHook(({ id }) => usePipelineRun(api, id, update), { initialProps: { id: first.id } });
    await advance();
    rerender({ id: second.id });
    await advance();
    expect(result.current.run?.id).toBe(second.id);
    expect(result.current.output.lines[0]?.text).toBe("second only");
    await act(async () => old.resolve(first));
    expect(result.current.run?.id).toBe(second.id);
    expect(update).toHaveBeenCalledExactlyOnceWith(second);
    unmount();
    await advance(5000);
    expect(api.getPipelineRun).toHaveBeenCalledTimes(2);
    expect(api.pipelineRunLogs).toHaveBeenCalledExactlyOnceWith(second.id, 0);
  });

  it("ignores a page resolving after unmount", async () => {
    const run = makeRun();
    const api = makeApi([], true, run.manifest, [], { runs: [run] });
    const logs = deferred<RunLogPage>();
    api.pipelineRunLogs.mockReturnValueOnce(logs.promise);
    const { unmount } = renderHook(() => usePipelineRun(api, run.id));
    await advance();
    unmount();
    await act(async () => logs.resolve({ entries: [makeLog("late")], nextCursor: 1, hasMore: true }));
    await advance(5000);
    expect(api.pipelineRunLogs).toHaveBeenCalledOnce();
  });
});
