import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useRegistry } from "./useRegistry";
import { consumerDeploymentFixture, consumerRegisteredFixture, makeApi, registeredFixture } from "../test/fixtures";
import type { RegisteredApp } from "../lib/types";

describe("registry state", () => {
  it("keeps optimistic registrations in the core's app-ID order, not display-name order", async () => {
    const first: RegisteredApp = {
      ...registeredFixture,
      manifest: { ...registeredFixture.manifest, appId: "a-first", name: "Zebra" },
    };
    const last: RegisteredApp = {
      ...registeredFixture,
      manifest: { ...registeredFixture.manifest, appId: "z-last", name: "Alpha" },
    };
    const api = makeApi([first]);
    const { result } = renderHook(() => useRegistry(api));
    await waitFor(() => expect(result.current.loading).toBe(false));
    api.listApps.mockImplementationOnce(() => new Promise<RegisteredApp[]>(() => {}));
    act(() => result.current.put(last));
    expect(result.current.snapshot?.apps.map((app) => app.manifest.appId)).toEqual(["a-first", "z-last"]);
  });

  it("retains deployment history when registration metadata is removed", async () => {
    const api = makeApi([consumerRegisteredFixture], true, consumerRegisteredFixture.manifest, [consumerDeploymentFixture]);
    const { result } = renderHook(() => useRegistry(api));
    await waitFor(() => expect(result.current.loading).toBe(false));
    api.listApps.mockImplementationOnce(() => new Promise<RegisteredApp[]>(() => {}));
    act(() => result.current.remove("atlas"));
    expect(result.current.snapshot?.apps).toEqual([]);
    expect(result.current.snapshot?.deployments).toEqual([consumerDeploymentFixture]);
    expect(api.recordConsumerDeployment).not.toHaveBeenCalled();
  });

  it("inserts a confirmed record once without losing earlier history", async () => {
    const api = makeApi([consumerRegisteredFixture], true, consumerRegisteredFixture.manifest, [consumerDeploymentFixture]);
    const { result } = renderHook(() => useRegistry(api));
    await waitFor(() => expect(result.current.loading).toBe(false));
    api.listApps.mockImplementation(() => new Promise<RegisteredApp[]>(() => {}));
    const next = { ...consumerDeploymentFixture, id: "2", releaseRef: "v1.5.1" };
    act(() => { result.current.putDeployment(next); result.current.putDeployment(next); });
    expect(result.current.snapshot?.deployments.map((record) => record.id)).toEqual(["2", "1"]);
  });
});
