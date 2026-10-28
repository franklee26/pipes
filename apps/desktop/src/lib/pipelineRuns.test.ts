import { describe, expect, it } from "vitest";
import { makeRun, promotionRegisteredFixture } from "../test/fixtures";
import { stageWaitKind } from "./pipelineRuns";

const stage = promotionRegisteredFixture.manifest.stages[1]!;
const run = makeRun({ manifest: promotionRegisteredFixture.manifest });
const command = run.commands.find((command) => command.stageId === stage.id)!;
const state = { ...run.stages[1]!, status: "waiting_approval" as const };

describe("stage wait classification", () => {
  it.each([
    { required: true, approved: null, criteria: true, commandStatus: "pending", expected: "local" },
    { required: true, approved: "2026-10-27T22:01:00Z", criteria: true, commandStatus: "pending", expected: "runner" },
    { required: true, approved: "2026-10-27T22:01:00Z", criteria: false, commandStatus: "passed", expected: "runner" },
    { required: true, approved: "2026-10-27T22:01:00Z", criteria: true, commandStatus: "passed", expected: "promotion" },
    { required: false, approved: null, criteria: true, commandStatus: "passed", expected: "promotion" },
    { required: false, approved: null, criteria: true, commandStatus: "running", expected: "runner" },
    { required: false, approved: null, criteria: false, commandStatus: "passed", expected: "runner" },
  ] as const)("classifies $expected with local approval $approved and command $commandStatus", ({ required, approved, criteria, commandStatus, expected }) => {
    expect(stageWaitKind(
      { ...stage, requiresApproval: required, promotionCriteria: criteria ? stage.promotionCriteria : [] },
      { ...state, approvedAt: approved },
      [{ ...command, status: commandStatus }],
    )).toBe(expected);
  });

  it("does not infer that unreported command states have passed", () => {
    expect(stageWaitKind(stage, state, [])).toBe("runner");
  });

  it("does not expose any approval gate for a stage that is no longer waiting", () => {
    expect(stageWaitKind({ ...stage, requiresApproval: true }, { ...state, status: "running", approvedAt: null }, [command])).toBeNull();
  });
});
