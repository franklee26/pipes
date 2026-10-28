import { describe, expect, it } from "vitest";
import { budgetLabel, buildCheckPlan, runBudget } from "./checkPlan";
import { composedPlanFixture, unitCommandFixture } from "../test/fixtures";
import type { CheckPlanNode } from "./types";

function command(id: string): CheckPlanNode {
  return { kind: "command", command: { ...unitCommandFixture, id } };
}

describe("check-plan presentation", () => {
  it("preserves parallel sets and the sequential boundary between them", () => {
    const result = buildCheckPlan(composedPlanFixture, 8);
    expect(result.commandCount).toBe(5);
    expect(result.steps.map((step) => [step.id, step.kind, step.commandCount])).toEqual([
      ["quick-tests", "parallel", 2],
      ["acceptance", "parallel", 3],
    ]);
    const acceptance = result.steps.find((step) => step.id === "acceptance");
    if (acceptance?.kind !== "parallel") throw new Error("Expected the acceptance group");
    expect(acceptance.budget).toEqual({ cap: 2, hostLimited: false });
    const sequence = acceptance.steps.find((step) => step.id === "package-checks");
    if (sequence?.kind !== "sequence") throw new Error("Expected an ordered branch");
    expect(sequence.steps.map((step) => step.id)).toEqual(["build", "inspect"]);
  });

  it("keeps auto unresolved instead of pinning registration-machine CPU capacity", () => {
    expect(runBudget("auto")).toEqual({ cap: null, hostLimited: true });
    expect(budgetLabel(runBudget("auto"))).toBe("auto / host capacity");
  });

  it("applies a group ceiling on top of auto host capacity", () => {
    const result = buildCheckPlan([
      { kind: "parallel", id: "checks", maxParallel: 4, steps: [command("one"), command("two")] },
    ], "auto");
    const group = result.steps[0];
    if (group?.kind !== "parallel") throw new Error("Expected a parallel group");
    expect(group.budget).toEqual({ cap: 4, hostLimited: true });
    expect(budgetLabel(group.budget)).toBe("max 4 / host-limited");
  });

  it("does not multiply limits or allow an inner group to raise its parent ceiling", () => {
    const result = buildCheckPlan([
      {
        kind: "parallel", id: "outer", maxParallel: 3,
        steps: [
          {
            kind: "sequence", id: "branch",
            steps: [{
              kind: "parallel", id: "inner", maxParallel: 12,
              steps: [command("one"), command("two")],
            }],
          },
        ],
      },
    ], 8);
    const outer = result.steps[0];
    if (outer?.kind !== "parallel") throw new Error("Expected outer group");
    const branch = outer.steps[0];
    if (branch?.kind !== "sequence") throw new Error("Expected ordered branch");
    const inner = branch.steps[0];
    if (inner?.kind !== "parallel") throw new Error("Expected inner group");
    expect(outer.budget.cap).toBe(3);
    expect(inner.budget.cap).toBe(3);
    expect(inner.requestedLimit).toBe(12);
  });

  it("uses a lower shared run budget even if a group declares a larger cap", () => {
    const result = buildCheckPlan([
      { kind: "parallel", id: "checks", maxParallel: 10, steps: [command("one")] },
    ], 2);
    const group = result.steps[0];
    if (group?.kind !== "parallel") throw new Error("Expected a parallel group");
    expect(group.budget.cap).toBe(2);
  });

  it("keeps legacy command nodes sequential and does not mutate input", () => {
    const input = [command("one"), command("two")];
    const before = structuredClone(input);
    const result = buildCheckPlan(input, "auto");
    expect(result.steps.map((step) => step.id)).toEqual(["one", "two"]);
    expect(result.commandCount).toBe(2);
    expect(input).toEqual(before);
  });

  it("accepts an empty stage plan but rejects empty groups", () => {
    expect(buildCheckPlan([], "auto").commandCount).toBe(0);
    expect(() => buildCheckPlan([{ kind: "sequence", id: "empty", steps: [] }], "auto")).toThrow("at least one");
    expect(() => buildCheckPlan([{ kind: "parallel", id: "empty", steps: [], maxParallel: null }], "auto")).toThrow("at least one");
  });

  it("rejects duplicate IDs across the complete stage tree", () => {
    expect(() => buildCheckPlan([
      { kind: "parallel", id: "outer", maxParallel: null, steps: [command("same")] },
      { kind: "sequence", id: "other", steps: [command("same")] },
    ], "auto")).toThrow("unique");
    expect(() => buildCheckPlan([
      { kind: "sequence", id: "same", steps: [command("same")] },
    ], "auto")).toThrow("unique");
  });

  it.each([0, -1, 1.5, NaN, Infinity, 2 ** 32])("rejects invalid concurrency %s", (limit) => {
    expect(() => runBudget(limit)).toThrow("positive integers");
    expect(() => buildCheckPlan([
      { kind: "parallel", id: "checks", maxParallel: limit, steps: [command("one")] },
    ], "auto")).toThrow("positive integers");
  });

  it("enforces the 32-group nesting bound before rendering", () => {
    let root = command("leaf");
    for (let index = 0; index < 32; index += 1) {
      root = { kind: "sequence", id: `level-${index}`, steps: [root] };
    }
    expect(buildCheckPlan([root], "auto").commandCount).toBe(1);
    expect(() => buildCheckPlan([
      { kind: "sequence", id: "too-deep", steps: [root] },
    ], "auto")).toThrow("32-level");
  });

  it("reports missing canonical cap metadata rather than treating it as inheritance", () => {
    const node: CheckPlanNode = {
      kind: "parallel", id: "checks", maxParallel: null, steps: [command("one")],
    };
    Reflect.deleteProperty(node, "maxParallel");
    expect(() => buildCheckPlan([node], "auto")).toThrow("positive integers");
  });
});
