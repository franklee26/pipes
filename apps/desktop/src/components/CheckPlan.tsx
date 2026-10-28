import { useMemo } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { budgetLabel, buildCheckPlan, runBudget, type DisplayCheck, type DisplayCheckPlan } from "../lib/checkPlan";
import type { CheckPlanNode, Parallelism } from "../lib/types";
import { CommandList } from "./CommandList";
import { ErrorBanner } from "./ErrorBanner";
import { Arrow } from "./Icons";

export function RunBudget({ value }: { value: Parallelism }) {
  try {
    return <span title="Maximum simultaneous commands, not test-tool internal threads.">{budgetLabel(runBudget(value))}</span>;
  } catch (failure) {
    return <ErrorBanner error={displayError(failure)} />;
  }
}

function CheckSequence({
  steps,
  depth,
  label,
  sourcePath,
}: {
  steps: DisplayCheck[];
  depth: number;
  label: string;
  sourcePath?: string;
}) {
  return (
    <ol className="check-sequence" aria-label={label}>
      {steps.map((node, index) => (
        <li className="check-sequence-step" key={node.id}>
          <CheckNode node={node} depth={depth} sourcePath={sourcePath} />
          {index < steps.length - 1 && (
            <div className="check-barrier">
              <span>all pass</span>{" "}<Arrow />{" "}<strong>then</strong>
            </div>
          )}
        </li>
      ))}
    </ol>
  );
}

function CheckNode({ node, depth, sourcePath }: { node: DisplayCheck; depth: number; sourcePath?: string }) {
  if (node.kind === "command") {
    return (
      <section className="check-command" aria-label={`${node.id} command`} data-check-id={node.id}>
        <CommandList commands={[node.command]} sourcePath={sourcePath} />
      </section>
    );
  }
  return (
    <details
      className={`check-group check-group-${node.kind}${depth >= 2 ? " check-group-deep" : ""}`}
      open={depth < 2}
      aria-label={`${node.id} ${node.kind} group`}
      data-check-id={node.id}
    >
      <summary className="check-group-heading">
        <span className="check-group-title">
          <strong>{node.id}</strong>
          <span className="check-mode">{node.kind === "parallel" ? "parallel / all" : "in order"}</span>
        </span>
        <span className="check-group-meta">
          {node.commandCount} {node.commandCount === 1 ? "check" : "checks"}
          {node.kind === "parallel" && <> / {budgetLabel(node.budget)}</>}
        </span>
      </summary>
      <div className="check-group-body">
        {node.kind === "parallel" ? (
          <>
            {node.requestedLimit !== null && node.budget.cap !== node.requestedLimit && (
              <p className="check-cap-note">Group cap {node.requestedLimit}; shared run and parent limits still apply.</p>
            )}
            <ul className="check-parallel" aria-label={`${node.id} concurrent branches`}>
              {node.steps.map((child) => <li key={child.id}><CheckNode node={child} depth={depth + 1} sourcePath={sourcePath} /></li>)}
            </ul>
          </>
        ) : (
          <CheckSequence steps={node.steps} depth={depth + 1} label={`${node.id} ordered checks`} sourcePath={sourcePath} />
        )}
      </div>
    </details>
  );
}

type PlanResult =
  | { kind: "ready"; plan: DisplayCheckPlan }
  | { kind: "error"; error: DisplayError };

export function CheckPlan({
  steps,
  parallelism,
  sourcePath,
}: {
  steps: CheckPlanNode[];
  parallelism: Parallelism;
  sourcePath?: string;
}) {
  const result = useMemo<PlanResult>(() => {
    try {
      return { kind: "ready", plan: buildCheckPlan(steps, parallelism) };
    } catch (failure) {
      return { kind: "error", error: displayError(failure) };
    }
  }, [steps, parallelism]);

  if (result.kind === "error") return <ErrorBanner error={result.error} />;
  if (result.plan.steps.length === 0) return <p className="compact-note">No checks configured for this stage.</p>;
  return (
    <section className="check-plan" aria-label="Stage check plan">
      <div className="check-plan-heading">
        <strong className="eyebrow">Check sequence</strong>
        <span className="muted" title="Maximum simultaneous commands, not test-tool internal threads.">shared budget: {budgetLabel(result.plan.budget)}</span>
      </div>
      <div className="check-plan-viewport">
        <CheckSequence steps={result.plan.steps} depth={0} label="Stage check sequence" sourcePath={sourcePath} />
      </div>
      <p className="check-plan-note">Every check is required. Plans only; no commands execute.</p>
    </section>
  );
}
