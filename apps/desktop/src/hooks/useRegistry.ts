import { useCallback, useEffect, useReducer, useState } from "react";
import { displayError, type DisplayError } from "../lib/api";
import type { ConsumerDeployment, HarnessResult, PipesApi, PromotionApproval, RegisteredApp, RegistryInfo } from "../lib/types";

interface Snapshot {
  apps: RegisteredApp[];
  info: RegistryInfo;
  deployments: ConsumerDeployment[];
  harnessResults: HarnessResult[];
  promotionApprovals: PromotionApproval[];
}

export function useRegistry(api: PipesApi) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [error, setError] = useState<DisplayError | null>(null);
  const [loading, setLoading] = useState(true);
  const [revision, refresh] = useReducer((value: number) => value + 1, 0);

  useEffect(() => {
    let current = true;
    if (!api.available) {
      setLoading(false);
      setError({
        code: "desktop_required",
        message: "Use the Pipes desktop app for the local registry. Start it with npm run dev.",
      });
      return;
    }
    setLoading(true);
    setError(null);
    void Promise.all([api.listApps(), api.registryInfo(), api.listConsumerDeployments(), api.listHarnessResults(), api.listPromotionApprovals()])
      .then(([apps, info, deployments, harnessResults, promotionApprovals]) => {
        if (current) setSnapshot({ apps, info, deployments, harnessResults, promotionApprovals });
      })
      .catch((failure: unknown) => {
        if (current) setError(displayError(failure));
      })
      .finally(() => {
        if (current) setLoading(false);
      });
    return () => { current = false; };
  }, [api, revision]);

  useEffect(() => {
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  const put = useCallback((app: RegisteredApp) => {
    setSnapshot((previous) => previous ? {
      ...previous,
      apps: [...previous.apps.filter((item) => item.manifest.appId !== app.manifest.appId), app]
        .sort((left, right) => {
          const leftId = left.manifest.appId;
          const rightId = right.manifest.appId;
          return leftId < rightId ? -1 : leftId > rightId ? 1 : 0;
        }),
    } : previous);
    refresh();
  }, []);

  const remove = useCallback((appId: string) => {
    setSnapshot((previous) => previous ? {
      ...previous,
      apps: previous.apps.filter((item) => item.manifest.appId !== appId),
    } : previous);
    refresh();
  }, []);

  const putDeployment = useCallback((record: ConsumerDeployment) => {
    setSnapshot((previous) => previous ? {
      ...previous,
      deployments: [record, ...previous.deployments.filter((item) => item.id !== record.id)],
    } : previous);
    refresh();
  }, []);

  const putHarnessResult = useCallback((result: HarnessResult) => {
    setSnapshot((previous) => previous ? {
      ...previous, harnessResults: [result, ...previous.harnessResults.filter((item) => item.id !== result.id)],
    } : previous);
    refresh();
  }, []);

  const putPromotionApproval = useCallback((approval: PromotionApproval) => {
    setSnapshot((previous) => previous ? {
      ...previous, promotionApprovals: [approval, ...previous.promotionApprovals.filter((item) => item.id !== approval.id)],
    } : previous);
    refresh();
  }, []);

  return { snapshot, error, loading, refresh, put, remove, putDeployment, putHarnessResult, putPromotionApproval };
}
