import { describe, expect, it } from "vitest";
import {
  getSampleProject,
  getSamplePromotionReview,
  sampleConsumerDeployments,
  sampleHarnessResults,
} from "./conceptPreview";
import { promotionContext } from "./promotion";
import { simulateHarnessResult, simulatePromotionApproval, simulatePromotionAssessment } from "./promotionPreview";
import type { ApprovePromotion, ConsumerDeployment, HarnessResult, PromotionCriterion } from "./types";

const review = getSamplePromotionReview("atlas", "gamma");
const seedResult = sampleHarnessResults[0]!;
const seedLab = sampleConsumerDeployments.find((record) => record.stageId === "gamma")!;
const sampleRecord = { id: "preview-new-record", recordedAt: "2026-10-26T12:31:00Z" };
const context = promotionContext(review, "v1.5.0");

function assessment(results: readonly HarnessResult[] = [seedResult], deployments: readonly ConsumerDeployment[] = [seedLab]) {
  return simulatePromotionAssessment(review, "v1.5.0", results, deployments);
}

function approvalRequest(overrides: Partial<ApprovePromotion> = {}): ApprovePromotion {
  return {
    ...context,
    expectedAssessmentHash: assessment().assessmentHash,
    approved: true,
    acknowledgeOptional: true,
    notes: null,
    ...overrides,
  };
}

function optionalDeployment(): ConsumerDeployment {
  const consumer = review.stage.consumers.find((item) => item.id === "personal-canary")!;
  return { ...seedLab, ...sampleRecord, consumer: { ...consumer } };
}

describe("preview promotion policy data", () => {
  it("declares the exact canary threshold, command leaf, and required/optional consumers only on Atlas gamma", () => {
    expect(review.stage.promotionCriteria).toEqual([
      { kind: "harness", id: "canary-tests", checkId: "canary", expectedTests: 50, minimumPassed: 45 },
      { kind: "deployment", id: "lab-delivery", consumerId: "lab-device", requirement: "required" },
      { kind: "deployment", id: "personal-delivery", consumerId: "personal-canary", requirement: "optional" },
    ]);
    expect(review.stage.checkPlan).toEqual([{ kind: "command", command: review.stage.checks[0] }]);
    expect(review.stage.checks[0]?.id).toBe("canary");
    const yaml = getSampleProject("atlas").manifest;
    expect(yaml).toContain("check: canary\n          expected_tests: 50\n          minimum_passed: 45");
    expect(yaml).toContain("consumer: lab-device\n          requirement: required");
    expect(yaml).toContain("consumer: personal-canary\n          requirement: optional");
    for (const id of ["beacon", "relay"] as const) {
      expect(getSampleProject(id).stages.every((stage) => stage.promotionCriteria.length === 0)).toBe(true);
      expect(getSampleProject(id).manifest).not.toContain("promotion:");
    }
    expect(sampleConsumerDeployments.find((record) => record.stageId === "beta")?.releaseRef).toBe("v1.4.2");
  });
});

describe("simulation-only exact n/m evaluation", () => {
  it.each([
    { passed: 44, total: 50, status: "blocked" },
    { passed: 45, total: 50, status: "satisfied" },
    { passed: 50, total: 50, status: "satisfied" },
    { passed: 45, total: 49, status: "blocked" },
    { passed: 50, total: 51, status: "blocked" },
    { passed: 0, total: 0, status: "blocked" },
    { passed: 0, total: 50, status: "blocked" },
  ])("evaluates $passed/$total as $status without percentage shortcuts", ({ passed, total, status }) => {
    const result = assessment([{ ...seedResult, passed, total }]);
    expect(result.criteria[0]?.status).toBe(status);
    expect(result.criteria[0]?.observed).toEqual({ passed, total });
    expect(result.status).toBe(status === "satisfied" ? "ready_with_warnings" : "blocked");
  });

  it("blocks missing reports rather than inferring totals or a pass", () => {
    const result = assessment([]);
    expect(result.status).toBe("blocked");
    expect(result.criteria[0]).toMatchObject({ status: "blocked", evidenceId: null, observed: null });
    expect(result.criteria[0]?.message).toContain("No current reported results");
  });

  it.each([
    { total: -1, passed: 0 }, { total: 50, passed: -1 },
    { total: 50, passed: 51 }, { total: 50.5, passed: 45 },
    { total: 50, passed: 45.5 }, { total: 0x1_0000_0000, passed: 45 },
    { total: 50, passed: Number.NaN }, { total: 50, passed: Infinity },
  ])("blocks invalid reported counts $passed/$total", ({ total, passed }) => {
    const result = assessment([{ ...seedResult, total, passed }]);
    expect(result.status).toBe("blocked");
    expect(result.criteria[0]?.message).toContain("invalid counts");
  });

  it.each([
    ["appId", "beacon"], ["stageId", "beta"], ["releaseRef", "v1.4.2"],
    ["manifestHash", "old-hash"], ["manifestPath", "/other/pipes.yaml"],
    ["sourcePath", "/other/source"], ["registrationUpdatedAt", "2026-10-25T10:00:00Z"],
  ] as const)("does not reuse harness evidence with a different %s", (key, value) => {
    const stale = { ...seedResult, [key]: value };
    expect(assessment([stale]).criteria[0]).toMatchObject({ status: "blocked", evidenceId: null });
  });

  it("uses the newest inserted matching report and ignores other candidates", () => {
    const failure = { ...seedResult, ...sampleRecord, passed: 44 };
    const otherCandidate = { ...failure, id: "preview-other", releaseRef: "v2.0.0", recordedAt: "2026-10-26T12:32:00Z" };
    expect(assessment([otherCandidate, failure, seedResult]).criteria[0]).toMatchObject({
      status: "blocked", evidenceId: failure.id, observed: { passed: 44, total: 50 },
    });
    expect(assessment([otherCandidate, seedResult]).criteria[0]?.status).toBe("satisfied");
    expect(assessment([failure, seedResult]).criteria[0]?.status).toBe("blocked");
  });

  it("does not reuse an older pass if the clock moves backwards for a new report", () => {
    const failure = { ...seedResult, id: "preview-new-failure", passed: 44, recordedAt: "2026-10-25T12:00:00Z" };
    expect(assessment([failure, seedResult]).criteria[0]).toMatchObject({
      status: "blocked", evidenceId: failure.id, observed: { passed: 44, total: 50 },
    });
  });

  it("does not fall back to an older passing report when the matching latest record is invalid", () => {
    for (const invalid of [
      { ...seedResult, ...sampleRecord, passed: 51 },
      { ...seedResult, ...sampleRecord, recordedAt: "invalid date" },
      { ...seedResult, ...sampleRecord, id: "" },
    ]) {
      expect(assessment([invalid, seedResult]).status).toBe("blocked");
    }
  });

  it("blocks required deployment gaps but warns about optional deployment gaps", () => {
    expect(assessment().criteria).toEqual(expect.arrayContaining([
      expect.objectContaining({ criterion: expect.objectContaining({ id: "lab-delivery" }), status: "satisfied", evidenceId: seedLab.id }),
      expect.objectContaining({ criterion: expect.objectContaining({ id: "personal-delivery" }), status: "warning", evidenceId: null }),
    ]));
    const missing = assessment([seedResult], []);
    expect(missing.status).toBe("blocked");
    expect(missing.criteria.find((item) => item.criterion.id === "lab-delivery")?.status).toBe("blocked");
    expect(missing.criteria.find((item) => item.criterion.id === "personal-delivery")?.status).toBe("warning");
    expect(assessment([seedResult], [seedLab, optionalDeployment()]).status).toBe("ready");
  });

  it.each([
    ["appId", "beacon"], ["stageId", "beta"], ["releaseRef", "v1.4.2"],
    ["manifestHash", "old-hash"], ["manifestPath", "/other/pipes.yaml"],
    ["sourcePath", "/other/source"], ["registrationUpdatedAt", "2026-10-25T10:00:00Z"],
  ] as const)("does not reuse deployment evidence with a different %s", (key, value) => {
    const stale = { ...seedLab, [key]: value };
    expect(assessment([seedResult], [stale]).criteria[1]).toMatchObject({ status: "blocked", evidenceId: null });
  });

  it("requires the current consumer definition and blocks malformed deployment evidence", () => {
    for (const consumer of [
      { ...seedLab.consumer, id: "other-device" },
      { ...seedLab.consumer, target: "another workstation" },
      { ...seedLab.consumer, name: "renamed workstation" },
    ]) {
      expect(assessment([seedResult], [{ ...seedLab, consumer }]).status).toBe("blocked");
    }
    expect(assessment([seedResult], [{ ...seedLab, recordedAt: "invalid" }]).status).toBe("blocked");
  });

  it("never turns an unknown optional consumer into an acknowledged skip", () => {
    const invalid = structuredClone(review);
    invalid.stage.promotionCriteria = [{
      id: "unknown-delivery", kind: "deployment", consumerId: "unknown", requirement: "optional",
    }];
    const result = simulatePromotionAssessment(invalid, "v1.5.0", [], []);
    expect(result.status).toBe("blocked");
    expect(result.criteria[0]).toMatchObject({ status: "blocked", consumer: null });
    expect(result.criteria[0]?.message).toContain("cannot be skipped");
  });

  it.each([
    { expectedTests: 0, minimumPassed: 0 },
    { expectedTests: 50, minimumPassed: 0 },
    { expectedTests: 50, minimumPassed: 51 },
    { expectedTests: 50.5, minimumPassed: 45 },
  ])("rejects invalid n/m policies $expectedTests/$minimumPassed", (counts) => {
    const invalid = structuredClone(review);
    invalid.stage.promotionCriteria = [{ kind: "harness", id: "canary-tests", checkId: "canary", ...counts }];
    expect(simulatePromotionAssessment(invalid, "v1.5.0", [seedResult], []).status).toBe("blocked");
  });

  it("requires a command leaf, not a missing command or a group ID", () => {
    const invalid = structuredClone(review);
    invalid.stage.checkPlan = [{ kind: "sequence", id: "canary", steps: [] }];
    expect(simulatePromotionAssessment(invalid, "v1.5.0", [seedResult], [seedLab]).criteria[0]?.message).toContain("not a command leaf");
  });

  it("treats omitted criteria as not configured rather than ready", () => {
    const result = simulatePromotionAssessment(getSamplePromotionReview("beacon", "gamma"), "v2.8.1", [], []);
    expect(result.status).toBe("not_configured");
    expect(result.criteria).toEqual([]);
  });

  it("rejects an empty or malformed candidate and includes provenance in its preview-only fingerprint", () => {
    for (const reference of ["", "  ", "v1.5.0\n", "a".repeat(257)]) {
      expect(() => simulatePromotionAssessment(review, reference, [seedResult], [seedLab])).toThrow();
    }
    const first = simulatePromotionAssessment(review, " v1.5.0 ", [seedResult], [seedLab]);
    expect(first.releaseRef).toBe("v1.5.0");
    expect(first.assessmentHash).toMatch(/^preview:[0-9a-f]{8}$/);
    expect(first.assessmentHash).toBe(assessment().assessmentHash);
    const missing = simulatePromotionAssessment(review, "other-ref", [], []);
    const changedPath = simulatePromotionAssessment({ ...review, manifestPath: "/other/pipes.yaml" }, "other-ref", [], []);
    expect(changedPath.assessmentHash).not.toBe(missing.assessmentHash);
  });
});

describe("in-memory report and approval records", () => {
  it("retains actual externally reported counts, including zero, and the reviewed source metadata", () => {
    const result = simulateHarnessResult(review, {
      ...context, checkId: "canary", total: 49, passed: 45, notes: "  sample external report  ",
    }, sampleRecord);
    expect(result).toMatchObject({
      ...sampleRecord, total: 49, passed: 45, notes: "sample external report",
      manifestHash: review.manifestHash, manifestPath: review.manifestPath, sourcePath: review.sourcePath,
      registrationUpdatedAt: review.updatedAt, appId: "atlas", stageId: "gamma", checkId: "canary", releaseRef: "v1.5.0",
    });
    const zero = simulateHarnessResult(review, { ...context, checkId: "canary", total: 0, passed: 0, notes: null }, sampleRecord);
    expect(zero).toMatchObject({ total: 0, passed: 0 });
    expect(assessment([zero]).status).toBe("blocked");
  });

  it("rejects reports with stale context, unknown checks, or invalid counts", () => {
    const request = { ...context, checkId: "canary", total: 50, passed: 45, notes: null };
    expect(() => simulateHarnessResult(review, { ...request, expectedUpdatedAt: "old" }, sampleRecord)).toThrow();
    expect(() => simulateHarnessResult(review, { ...request, checkId: "missing" }, sampleRecord)).toThrow();
    expect(() => simulateHarnessResult(review, { ...request, passed: 51 }, sampleRecord)).toThrow();
  });

  it.each([
    { approved: false, acknowledgeOptional: false },
    { approved: false, acknowledgeOptional: true },
    { approved: true, acknowledgeOptional: false },
  ])("requires both explicit approval and acknowledgement for optional skips: %o", (flags) => {
    expect(() => simulatePromotionApproval(review, approvalRequest(flags), [seedResult], [seedLab], sampleRecord)).toThrow();
  });

  it("re-evaluates before approval and rejects a stale hash instead of reusing an earlier passing assessment", () => {
    const failure = { ...seedResult, ...sampleRecord, passed: 44 };
    expect(() => simulatePromotionApproval(review, approvalRequest(), [failure, seedResult], [seedLab], sampleRecord))
      .toThrow(expect.objectContaining({ code: "assessment_changed" }));
    const blocked = assessment([failure]);
    expect(() => simulatePromotionApproval(review, approvalRequest({ expectedAssessmentHash: blocked.assessmentHash }), [failure], [seedLab], sampleRecord))
      .toThrow(expect.objectContaining({ code: "promotion_blocked" }));
    expect(() => simulatePromotionApproval(review, approvalRequest({ expectedManifestHash: "old" }), [seedResult], [seedLab], sampleRecord))
      .toThrow(expect.objectContaining({ code: "registration_changed" }));
  });

  it("freezes skipped warnings even when optional delivery is later recorded or definitions change", () => {
    const currentReview = structuredClone(review);
    const prior = simulatePromotionApproval(currentReview, approvalRequest(), [seedResult], [seedLab], sampleRecord);
    expect(prior.skippedOptional).toEqual(["personal-delivery"]);
    expect(prior.assessment.status).toBe("ready_with_warnings");
    const later = simulatePromotionAssessment(currentReview, "v1.5.0", [seedResult], [seedLab, optionalDeployment()]);
    expect(later.status).toBe("ready");
    expect(later.assessmentHash).not.toBe(prior.assessment.assessmentHash);
    const criterion = currentReview.stage.promotionCriteria[2]!;
    criterion.id = "new-criterion-name";
    expect(prior.skippedOptional).toEqual(["personal-delivery"]);
    expect(prior.assessment.criteria[2]).toMatchObject({ status: "warning", evidenceId: null, criterion: { id: "personal-delivery" } });
  });

  it("records no skips when every rule is satisfied and never approves unconfigured policy", () => {
    const deployments = [seedLab, optionalDeployment()];
    const ready = assessment([seedResult], deployments);
    expect(simulatePromotionApproval(review, approvalRequest({
      expectedAssessmentHash: ready.assessmentHash, acknowledgeOptional: false,
    }), [seedResult], deployments, sampleRecord).skippedOptional).toEqual([]);
    const empty = { ...review, stage: { ...review.stage, promotionCriteria: [] as PromotionCriterion[] } };
    const unconfigured = simulatePromotionAssessment(empty, "v1.5.0", [], []);
    expect(() => simulatePromotionApproval(empty, approvalRequest({ expectedAssessmentHash: unconfigured.assessmentHash }), [], [], sampleRecord))
      .toThrow(expect.objectContaining({ code: "promotion_not_configured" }));
  });
});
