import type { ConsumerDeployment, ConsumerSummary, RegisteredApp } from "./types";

export interface DeploymentReview {
  appId: string;
  appName: string;
  stageId: string;
  consumer: ConsumerSummary;
  manifestHash: string;
  manifestPath: string;
  sourcePath: string;
  updatedAt: string;
}

export function reviewConsumerDeployment(
  app: RegisteredApp,
  stageId: string,
  consumer: ConsumerSummary,
): DeploymentReview {
  return {
    appId: app.manifest.appId,
    appName: app.manifest.name,
    stageId,
    consumer: { ...consumer },
    manifestHash: app.manifest.manifestHash,
    manifestPath: app.manifest.manifestPath,
    sourcePath: app.manifest.sourcePath,
    updatedAt: app.updatedAt,
  };
}

export function deploymentsForStage(
  records: readonly ConsumerDeployment[],
  appId: string,
  stageId: string,
): ConsumerDeployment[] {
  return records.filter((record) => record.appId === appId && record.stageId === stageId);
}

export function latestConsumerDeployment(
  records: readonly ConsumerDeployment[],
  appId: string,
  stageId: string,
  consumer: ConsumerSummary,
): ConsumerDeployment | undefined {
  return records.find((record) =>
    record.appId === appId
    && record.stageId === stageId
    && record.consumer.id === consumer.id
    && record.consumer.target === consumer.target);
}
