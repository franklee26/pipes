import { noteProblem, releaseReferenceProblem } from "./inputValidation";
import { promotionReady, type PromotionReview } from "./promotion";
import type {
  ApprovePromotion,
  CheckPlanNode,
  ConsumerDeployment,
  CriterionEvaluation,
  HarnessResult,
  PromotionApproval,
  PromotionAssessment,
  PromotionContext,
  RecordHarnessResult,
} from "./types";

type PreviewRecord = Pick<HarnessResult, "id" | "recordedAt">;

function isCount(value: number): boolean {
  return Number.isInteger(value) && value >= 0 && value <= 0xffff_ffff;
}

function hasCommand(plan: readonly CheckPlanNode[], checkId: string): boolean {
  return plan.some((node) => node.kind === "command"
    ? node.command.id === checkId
    : hasCommand(node.steps, checkId));
}

function latestMatching<T extends PreviewRecord>(records: readonly T[], matches: (record: T) => boolean): T | undefined {
  // Like registry histories, preview records are newest-insertion first, not clock ordered.
  return records.find(matches);
}

function validRecord(record: PreviewRecord): boolean {
  return Boolean(record.id.trim()) && Number.isFinite(Date.parse(record.recordedAt));
}

function checkReference(releaseRef: string): string {
  const reference = releaseRef.trim();
  const problem = releaseReferenceProblem(releaseRef);
  if (!reference || problem) {
    throw { code: "invalid_promotion", message: problem ?? "Enter a candidate reference for this simulation." };
  }
  return reference;
}

function checkContext(review: PromotionReview, request: PromotionContext): void {
  if (request.appId !== review.appId || request.stageId !== review.stage.id
    || request.expectedManifestHash !== review.manifestHash || request.expectedUpdatedAt !== review.updatedAt) {
    throw { code: "registration_changed", message: "The sample configuration changed. Review it again before recording." };
  }
}

function previewFingerprint(value: unknown): string {
  let hash = 0x811c9dc5;
  for (const character of JSON.stringify(value)) {
    hash = Math.imul(hash ^ character.charCodeAt(0), 0x01000193);
  }
  return `preview:${(hash >>> 0).toString(16).padStart(8, "0")}`;
}

/** Simulation only. The native registry is authoritative outside the design preview. */
export function simulatePromotionAssessment(
  review: PromotionReview,
  releaseRef: string,
  results: readonly HarnessResult[],
  deployments: readonly ConsumerDeployment[],
): PromotionAssessment {
  const reference = checkReference(releaseRef);
  const matchingContext = (record: HarnessResult | ConsumerDeployment) =>
    record.appId === review.appId && record.stageId === review.stage.id
    && record.releaseRef === reference && record.manifestHash === review.manifestHash
    && record.manifestPath === review.manifestPath && record.sourcePath === review.sourcePath
    && record.registrationUpdatedAt === review.updatedAt;
  const criteria = review.stage.promotionCriteria.map((definition): CriterionEvaluation => {
    const criterion = { ...definition };
    if (criterion.kind === "harness") {
      const evidence = latestMatching(results, (result) => result.checkId === criterion.checkId && matchingContext(result));
      const validPolicy = isCount(criterion.expectedTests) && isCount(criterion.minimumPassed)
        && criterion.minimumPassed >= 1 && criterion.minimumPassed <= criterion.expectedTests;
      const knownCheck = hasCommand(review.stage.checkPlan, criterion.checkId);
      const validEvidence = evidence && validRecord(evidence) && isCount(evidence.total)
        && isCount(evidence.passed) && evidence.passed <= evidence.total;
      const satisfied = Boolean(validPolicy && knownCheck && validEvidence
        && evidence.total === criterion.expectedTests && evidence.passed >= criterion.minimumPassed);
      return {
        criterion,
        status: satisfied ? "satisfied" : "blocked",
        message: !validPolicy ? "Invalid sample harness policy: require 1 <= minimum passed <= expected tests <= 4294967295."
          : !knownCheck ? `Harness ${criterion.checkId} is not a command leaf in ${review.stage.id}.`
          : !evidence ? `No current reported results for ${criterion.checkId} and ${reference}.`
          : !validEvidence ? `The latest report for ${criterion.checkId} has invalid counts or record metadata; report it again.`
          : evidence.total !== criterion.expectedTests ? `Expected exactly ${criterion.expectedTests} tests; ${evidence.total} were reported.`
          : satisfied ? `${evidence.passed}/${evidence.total} passed; minimum ${criterion.minimumPassed} met.`
          : `${evidence.passed}/${evidence.total} passed; at least ${criterion.minimumPassed} must pass.`,
        evidenceId: evidence?.id ?? null,
        observed: validEvidence ? { total: evidence.total, passed: evidence.passed } : null,
        consumer: null,
      };
    }
    const consumer = review.stage.consumers.find((item) => item.id === criterion.consumerId);
    const evidence = consumer && latestMatching(deployments, (record) =>
      record.consumer.id === consumer.id && record.consumer.name === consumer.name
      && record.consumer.target === consumer.target && record.consumer.approval === consumer.approval
      && matchingContext(record));
    const validEvidence = evidence && validRecord(evidence);
    return {
      criterion,
      status: !consumer || (evidence && !validEvidence) ? "blocked"
        : validEvidence ? "satisfied" : criterion.requirement === "required" ? "blocked" : "warning",
      message: !consumer ? `Consumer ${criterion.consumerId} is not declared in ${review.stage.id}; this criterion cannot be skipped.`
        : evidence && !validEvidence ? `The latest deployment marker for ${consumer.name} has invalid record metadata.`
        : validEvidence ? `Manual deployment to ${consumer.name} is recorded for ${reference}.`
        : `${criterion.requirement === "optional" ? "Optional" : "Required"} deployment to ${consumer.name} has no current marker for ${reference}.`,
      evidenceId: evidence?.id ?? null,
      observed: null,
      consumer: consumer ? { ...consumer } : null,
    };
  });
  const status = criteria.length === 0 ? "not_configured"
    : criteria.some((item) => item.status === "blocked") ? "blocked"
    : criteria.some((item) => item.status === "warning") ? "ready_with_warnings" : "ready";
  const assessment: Omit<PromotionAssessment, "assessmentHash"> = {
    appId: review.appId,
    appName: review.appName,
    stageId: review.stage.id,
    releaseRef: reference,
    manifestHash: review.manifestHash,
    manifestPath: review.manifestPath,
    sourcePath: review.sourcePath,
    registrationUpdatedAt: review.updatedAt,
    status,
    criteria,
    stageRequirements: [...review.stage.requires],
  };
  return { ...assessment, assessmentHash: previewFingerprint(assessment) };
}

export function simulateHarnessResult(
  review: PromotionReview,
  request: RecordHarnessResult,
  record: PreviewRecord,
): HarnessResult {
  checkContext(review, request);
  const reference = checkReference(request.releaseRef);
  if (!hasCommand(review.stage.checkPlan, request.checkId)) {
    throw { code: "check_not_found", message: "The reported sample harness is not a command in this stage." };
  }
  if (!isCount(request.total) || !isCount(request.passed) || request.passed > request.total || !validRecord(record)) {
    throw { code: "invalid_harness_result", message: "Sample results need valid whole-number counts and record metadata." };
  }
  const problem = noteProblem(request.notes ?? "");
  if (problem) throw { code: "invalid_harness_result", message: problem };
  return {
    ...record,
    appId: review.appId,
    appName: review.appName,
    stageId: review.stage.id,
    checkId: request.checkId,
    releaseRef: reference,
    total: request.total,
    passed: request.passed,
    notes: request.notes?.trim() || null,
    manifestHash: review.manifestHash,
    manifestPath: review.manifestPath,
    sourcePath: review.sourcePath,
    registrationUpdatedAt: review.updatedAt,
  };
}

export function simulatePromotionApproval(
  review: PromotionReview,
  request: ApprovePromotion,
  results: readonly HarnessResult[],
  deployments: readonly ConsumerDeployment[],
  record: PreviewRecord,
): PromotionApproval {
  checkContext(review, request);
  const assessment = simulatePromotionAssessment(review, request.releaseRef, results, deployments);
  if (assessment.assessmentHash !== request.expectedAssessmentHash) {
    throw { code: "assessment_changed", message: "Sample evidence or configuration changed. Assess readiness again before approving." };
  }
  if (!promotionReady(assessment)) {
    throw {
      code: assessment.status === "not_configured" ? "promotion_not_configured" : "promotion_blocked",
      message: "The current sample criteria do not permit this approval.",
    };
  }
  const skippedOptional = assessment.criteria.filter((criterion) => criterion.status === "warning").map((criterion) => criterion.criterion.id);
  if (!request.approved || (skippedOptional.length > 0 && !request.acknowledgeOptional)) {
    throw { code: "approval_required", message: "Explicitly approve the sample decision and acknowledge every skipped optional step." };
  }
  const problem = noteProblem(request.notes ?? "");
  if (problem || !validRecord(record)) {
    throw { code: "invalid_promotion", message: problem ?? "The sample approval record metadata is invalid." };
  }
  return { ...record, assessment: structuredClone(assessment), skippedOptional, notes: request.notes?.trim() || null };
}
