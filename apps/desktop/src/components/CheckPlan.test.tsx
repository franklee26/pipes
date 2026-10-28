import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { CheckPlan, RunBudget } from "./CheckPlan";
import { composedPlanFixture, unitCommandFixture } from "../test/fixtures";
import type { CheckPlanNode } from "../lib/types";

describe("stage check sequence", () => {
  it("shows parallel groups with an all-pass THEN barrier, including ordered branches", () => {
    render(<CheckPlan steps={composedPlanFixture} parallelism={8} />);
    const root = screen.getByRole("list", { name: "Stage check sequence" });
    expect(root.children).toHaveLength(2);
    const first = root.children.item(0);
    const second = root.children.item(1);
    if (!(first instanceof HTMLElement) || !(second instanceof HTMLElement)) throw new Error("Missing sequence entries");
    expect(within(first).getByRole("group", { name: "quick-tests parallel group" })).toBeVisible();
    expect(first.querySelector(":scope > .check-barrier")).toHaveTextContent("all pass then");
    expect(second.querySelector(":scope > .check-barrier")).toBeNull();
    const acceptance = within(second).getByRole("group", { name: "acceptance parallel group" });
    expect(within(acceptance).getByRole("list", { name: "acceptance concurrent branches" }).children).toHaveLength(2);
    const branch = within(acceptance).getByRole("list", { name: "package-checks ordered checks" });
    expect(branch.children).toHaveLength(2);
    expect(within(branch).getByText("then")).toBeVisible();
    expect(within(acceptance).getByText("3 checks / max 2 commands")).toBeVisible();
    expect(screen.getByText("Every check is required. Plans only; no commands execute.")).toBeVisible();
  });

  it("labels auto and inherited group ceilings without claiming a CPU count", () => {
    render(<CheckPlan steps={composedPlanFixture} parallelism="auto" />);
    expect(screen.getByText("shared budget: auto / host capacity")).toBeVisible();
    expect(screen.getByText("2 checks / auto / host capacity")).toBeVisible();
    expect(screen.getByText("3 checks / max 2 / host-limited")).toBeVisible();
  });

  it("explains when the shared budget lowers a declared group cap", () => {
    render(<CheckPlan steps={composedPlanFixture} parallelism={1} />);
    const acceptance = screen.getByRole("group", { name: "acceptance parallel group" });
    expect(within(acceptance).getByText("3 checks / max 1 command")).toBeVisible();
    expect(within(acceptance).getByText("Group cap 2; shared run and parent limits still apply.")).toBeVisible();
  });

  it("keeps command metadata available inside parallel groups", async () => {
    const user = userEvent.setup();
    const steps: CheckPlanNode[] = [{
      kind: "parallel", id: "metadata", maxParallel: null,
      steps: [{
        kind: "command",
        command: {
          ...unitCommandFixture,
          cwd: "subproject",
          timeout: "2m",
          env: { MODE: "test" },
          secrets: { TOKEN: "env://TEST_TOKEN" },
        },
      }],
    }];
    render(<CheckPlan steps={steps} parallelism={4} />);
    expect(screen.getByText('["cargo","test"]')).toBeVisible();
    expect(screen.getByText("cwd: subproject / timeout: 2m")).toBeVisible();
    await user.click(screen.getByText("environment", { exact: true }));
    await user.click(screen.getByText("credential references", { exact: true }));
    expect(screen.getByText(/"MODE": "test"/)).toBeVisible();
    expect(screen.getByText(/env:\/\/TEST_TOKEN/)).toBeVisible();
  });

  it("lets groups collapse without changing their ordering or requirement semantics", async () => {
    const user = userEvent.setup();
    render(<CheckPlan steps={composedPlanFixture} parallelism={8} />);
    const group = screen.getByRole("group", { name: "quick-tests parallel group" });
    expect(group).toHaveAttribute("open");
    await user.click(within(group).getByText("quick-tests", { exact: true }));
    expect(group).not.toHaveAttribute("open");
    expect(screen.getAllByText("then")[0]).toBeVisible();
  });

  it("renders legacy sequential checks as ordered commands", () => {
    render(<CheckPlan parallelism="auto" steps={[
      { kind: "command", command: unitCommandFixture },
      { kind: "command", command: { ...unitCommandFixture, id: "second" } },
    ]} />);
    expect(screen.getByRole("list", { name: "Stage check sequence" }).children).toHaveLength(2);
    expect(screen.getByText("then")).toBeVisible();
    expect(screen.queryByText("parallel / all")).not.toBeInTheDocument();
  });

  it("surfaces invalid metadata rather than showing an empty successful plan", () => {
    render(<CheckPlan steps={[{ kind: "sequence", id: "bad", steps: [] }]} parallelism="auto" />);
    expect(screen.getByRole("alert")).toHaveTextContent("at least one check");
    expect(screen.queryByRole("list", { name: "Stage check sequence" })).not.toBeInTheDocument();
  });

  it("renders invalid run-budget metadata as a visible error", () => {
    render(<RunBudget value={0} />);
    expect(screen.getByRole("alert")).toHaveTextContent("positive integers");
  });
});
