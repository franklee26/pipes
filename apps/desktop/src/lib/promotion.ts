import type { HarnessResult, PromotionAssessment, PromotionContext, RegisteredApp, StageSummary } from "./types";

export interface PromotionReview {
  appId: string;
  appName: string;
  stage: StageSummary;
  manifestHash: string;
  manifestPath: string;
  sourcePath: string;
  updatedAt: string;
}

export function reviewPromotion(app: RegisteredApp, stage: StageSummary): PromotionReview {
  return {
    appId: app.manifest.appId,
    appName: app.manifest.name,
    stage: structuredClone(stage),
    manifestHash: app.manifest.manifestHash,
    manifestPath: app.manifest.manifestPath,
    sourcePath: app.manifest.sourcePath,
    updatedAt: app.updatedAt,
  };
}

export function promotionContext(review: PromotionReview, releaseRef: string): PromotionContext {
  return {
    appId: review.appId,
    stageId: review.stage.id,
    expectedManifestHash: review.manifestHash,
    expectedUpdatedAt: review.updatedAt,
    releaseRef: releaseRef.trim(),
  };
}

export function promotionReady(assessment: PromotionAssessment): boolean {
  return assessment.status === "ready" || assessment.status === "ready_with_warnings";
}

export function promotionStatusLabel(assessment: PromotionAssessment): string {
  switch (assessment.status) {
    case "ready": return "criteria satisfied";
    case "ready_with_warnings": return "eligible with warnings";
    case "blocked": return "promotion criteria blocked";
    case "not_configured": return "no promotion criteria";
  }
}

export function harnessResultsForStage(results: readonly HarnessResult[], appId: string, stageId: string): HarnessResult[] {
  return results.filter((result) => result.appId === appId && result.stageId === stageId);
}
