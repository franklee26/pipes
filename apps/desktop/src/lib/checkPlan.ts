import type { CheckPlanNode, CommandSummary, Parallelism } from "./types";

export interface CommandBudget {
  cap: number | null;
  hostLimited: boolean;
}

export type DisplayCheck =
  | { kind: "command"; id: string; command: CommandSummary; commandCount: number }
  | { kind: "sequence"; id: string; steps: DisplayCheck[]; commandCount: number }
  | {
      kind: "parallel";
      id: string;
      steps: DisplayCheck[];
      commandCount: number;
      budget: CommandBudget;
      requestedLimit: number | null;
    };

export interface DisplayCheckPlan {
  steps: DisplayCheck[];
  budget: CommandBudget;
  commandCount: number;
}

class InvalidCheckPlan extends Error {
  readonly code = "invalid_check_plan";
}

function checkLimit(value: number): void {
  if (!Number.isInteger(value) || value < 1 || value > 0xffff_ffff) {
    throw new InvalidCheckPlan("Command limits must be positive integers within the supported range.");
  }
}

export function runBudget(parallelism: Parallelism): CommandBudget {
  if (parallelism === "auto") return { cap: null, hostLimited: true };
  checkLimit(parallelism);
  return { cap: parallelism, hostLimited: false };
}

export function budgetLabel(budget: CommandBudget): string {
  if (budget.cap === null) return "auto / host capacity";
  return budget.hostLimited
    ? `max ${budget.cap} / host-limited`
    : `max ${budget.cap} ${budget.cap === 1 ? "command" : "commands"}`;
}

export function buildCheckPlan(
  plan: readonly CheckPlanNode[],
  parallelism: Parallelism,
): DisplayCheckPlan {
  if (!Array.isArray(plan)) {
    throw new InvalidCheckPlan("Check-plan metadata is missing. Update and restart Pipes.");
  }
  const budget = runBudget(parallelism);
  const ids = new Set<string>();

  function visit(node: CheckPlanNode, inherited: CommandBudget, depth: number): DisplayCheck {
    if (!node || typeof node !== "object") {
      throw new InvalidCheckPlan("A check-plan entry is missing.");
    }
    const id = node.kind === "command" ? node.command?.id : node.id;
    if (typeof id !== "string" || id.trim() === "" || ids.has(id)) {
      throw new InvalidCheckPlan(`Check and group IDs must be unique: "${id}".`);
    }
    ids.add(id);
    if (node.kind === "command") {
      if (!Array.isArray(node.command.exec) || node.command.exec.length === 0) {
        throw new InvalidCheckPlan(`Check "${id}" has no command.`);
      }
      return { kind: "command", id, command: node.command, commandCount: 1 };
    }
    if (node.kind !== "sequence" && node.kind !== "parallel") {
      throw new InvalidCheckPlan(`Check "${id}" has an unknown plan kind.`);
    }
    if (depth >= 32) throw new InvalidCheckPlan("Check groups exceed the 32-level nesting limit.");
    if (!Array.isArray(node.steps) || node.steps.length === 0) {
      throw new InvalidCheckPlan(`Group "${id}" must contain at least one check.`);
    }
    let childBudget = inherited;
    if (node.kind === "parallel" && node.maxParallel !== null) {
      checkLimit(node.maxParallel);
      childBudget = {
        cap: inherited.cap === null ? node.maxParallel : Math.min(inherited.cap, node.maxParallel),
        hostLimited: inherited.hostLimited,
      };
    }
    const steps = node.steps.map((child) => visit(child, childBudget, depth + 1));
    const commandCount = steps.reduce((total, child) => total + child.commandCount, 0);
    return node.kind === "parallel"
      ? { kind: "parallel", id, steps, commandCount, budget: childBudget, requestedLimit: node.maxParallel }
      : { kind: "sequence", id, steps, commandCount };
  }

  const steps = plan.map((node) => visit(node, budget, 0));
  return { steps, budget, commandCount: steps.reduce((total, node) => total + node.commandCount, 0) };
}
