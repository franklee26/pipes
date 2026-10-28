import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { StageFlow, type StageFlowPreview, type StageTelemetry, type StageTone } from "./StageFlow";
import { layoutStages, type GraphDirection, type StageGraphVariant } from "../lib/stageGraph";
import type { StageSummary } from "../lib/types";
import { consumerFixture } from "../test/fixtures";

const stages: StageSummary[] = [
  { id: "test", after: [], checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [] },
  { id: "linux", after: ["test"], checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [] },
  { id: "macos", after: ["test"], checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [] },
  { id: "prod", after: ["linux", "macos"], checks: [], checkPlan: [], requires: ["previous.passed"], requiresApproval: false, consumers: [], promotionCriteria: [] },
];

function stage(id: string, after: string[] = []): StageSummary {
  return { id, after, checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [] };
}

function telemetry(overrides: Partial<StageTelemetry> = {}): StageTelemetry {
  return {
    tone: "idle",
    role: "preview",
    status: "locked",
    duration: "--:--",
    summary: "checks not started",
    progress: null,
    detail: "sample only",
    metric: "simulated",
    ...overrides,
  };
}

function stageElement(container: HTMLElement, id: string): HTMLElement {
  const element = Array.from(container.querySelectorAll<HTMLElement>(".stage-step"))
    .find((element) => element.dataset.stageId === id);
  if (!element) throw new Error(`Missing rendered stage: ${id}`);
  return element;
}

afterEach(() => vi.unstubAllGlobals());

describe("branching stage view", () => {
  it("draws declared fork and join connections, not links between adjacent array entries", () => {
    const { container } = render(<StageFlow stages={stages} />);
    const edges = Array.from(container.querySelectorAll(".stage-arrow"))
      .map((edge) => `${edge.getAttribute("data-from")}->${edge.getAttribute("data-to")}`);
    expect(edges).toEqual(["test->linux", "test->macos", "linux->prod", "macos->prod"]);
    expect(screen.getByText("join / all")).toBeVisible();
    expect(screen.getAllByText("not run")).toHaveLength(4);
    expect(container.querySelectorAll(".pipe-track")).toHaveLength(4);
    expect(container.querySelectorAll(".pipe-base")).toHaveLength(4);
    expect(container.querySelectorAll(".pipe-stream")).toHaveLength(4);
    expect(container.querySelectorAll(".pipe-droplet")).toHaveLength(0);
  });

  it("accepts real run telemetry without marking it as a simulation", () => {
    const { container } = render(<StageFlow stages={stages} telemetry={{
      stages: { test: telemetry({ tone: "running", status: "running", summary: "1/2 commands passed", progress: 50, detail: "recorded execution", metric: "1 finished" }) },
      transitions: [],
    }} />);
    expect(screen.getByRole("group", { name: "test: running" })).toBeVisible();
    expect(screen.getByText("1/2 commands passed")).toBeVisible();
    expect(container.querySelector('[data-preview="true"]')).toBeNull();
  });
  it("keeps stages keyboard-operable and describes all incoming dependencies", async () => {
    const select = vi.fn();
    const user = userEvent.setup();
    render(<StageFlow stages={stages} selectedStage="prod" onSelectStage={select} />);
    const target = screen.getByRole("button", { name: "prod: not run" });
    expect(target).toHaveAccessibleDescription("After all of: linux, macos.");
    expect(target).toHaveAttribute("aria-pressed", "true");
    target.focus();
    await user.keyboard("{Enter}");
    expect(select).toHaveBeenCalledWith("prod");
  });

  it("uses separate arrow markers in separate pipeline instances", () => {
    const { container } = render(
      <>
        <StageFlow stages={stages} label="First pipeline" />
        <StageFlow stages={stages} label="Second pipeline" />
      </>,
    );
    const ids = Array.from(container.querySelectorAll("marker")).map((marker) => marker.id);
    expect(ids).toHaveLength(2);
    expect(new Set(ids).size).toBe(2);
    expect(within(screen.getByRole("list", { name: "First pipeline" })).getAllByRole("listitem")).toHaveLength(4);
  });

  it("surfaces invalid graph metadata as an error instead of silently changing it", () => {
    render(<StageFlow stages={[{
      id: "test", after: ["missing"], checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [],
    }]} />);
    expect(screen.getByRole("alert")).toHaveTextContent("missing stage");
    expect(screen.queryByRole("list")).not.toBeInTheDocument();
  });

  it("shows honest planned telemetry, never execution inferred from requirements or approval", () => {
    const checks = ["lint", "unit"].map((id) => ({
      id, exec: ["npm", "test"], cwd: null, timeout: null, env: {}, secrets: {},
    }));
    const configured = [
      stage("verify"),
      {
        ...stage("prod", ["verify"]),
        checks,
        requires: ["previous.passed"],
        requiresApproval: true,
      },
    ];
    const { container } = render(<StageFlow stages={configured} onSelectStage={vi.fn()} />);
    expect(screen.getByRole("button", { name: "prod: not run" })).toHaveClass("idle");
    expect(screen.getByText("2 checks planned")).toBeVisible();
    expect(screen.getByText("manual approval")).toBeVisible();
    expect(screen.getByText("1 rule planned")).toBeVisible();
    expect(screen.getByText("no approval gate")).toBeVisible();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    expect(screen.queryByText(/0%|passed|running|waiting|gate unlocked/i)).not.toBeInTheDocument();
    expect(container.querySelectorAll(".stage-node.idle")).toHaveLength(2);
    expect(container.querySelector(".stage-node.idle .status-icon rect")).toBeNull();
    for (const meter of container.querySelectorAll<HTMLElement>(".stage-meter-fill")) {
      expect(meter).toHaveStyle({ width: "0%" });
    }
    expect(container.querySelector(".stage-time")).toHaveTextContent("--:--");
    expect(container.querySelector(".stage-header .stage-index")).toHaveTextContent("01");
    expect(container.querySelector(".stage-body .stage-name")).toHaveTextContent("verify");
    expect(container.querySelectorAll(".stage-footer .stage-submeta > span")).toHaveLength(4);
  });

  it("numbers declarations and exposes only actual ports even with forward references and independent endings", () => {
    const configured = [
      stage("first-end", ["first"]),
      stage("second"),
      stage("standalone"),
      stage("first"),
      stage("second-end", ["second"]),
    ];
    const { container } = render(<StageFlow stages={configured} selectedStage="standalone" />);
    const firstEnd = stageElement(container, "first-end");
    expect(within(firstEnd).getByText("01")).toBeVisible();
    expect(firstEnd.querySelectorAll(".stage-port")).toHaveLength(1);
    expect(firstEnd.querySelector('[data-port="incoming"]')).not.toBeNull();
    const first = stageElement(container, "first");
    expect(within(first).getByText("04")).toBeVisible();
    expect(within(first).getByText("entry")).toBeVisible();
    expect(first.querySelectorAll(".stage-port")).toHaveLength(1);
    expect(first.querySelector('[data-port="outgoing"]')).not.toBeNull();
    expect(stageElement(container, "second").querySelector('[data-port="incoming"]')).toBeNull();
    expect(stageElement(container, "second-end").querySelector('[data-port="outgoing"]')).toBeNull();
    expect(stageElement(container, "standalone").querySelector(".stage-port")).toBeNull();
    expect(screen.getByRole("group", { name: "standalone: not run" })).toHaveClass("selected");
  });

  it.each<GraphDirection>(["LR", "TB"])("renders sockets at the routed edge endpoints in %s", (direction) => {
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(direction === "LR" ? 900 : 600);
    const { container } = render(<StageFlow stages={stages} />);
    const graph = layoutStages(stages, direction);
    expect(container.querySelector(".graph-canvas")).toHaveAttribute("data-direction", direction);
    for (const edge of graph.edges) {
      const source = stageElement(container, edge.from);
      const target = stageElement(container, edge.to);
      const output = source.querySelector<HTMLElement>('[data-port="outgoing"]')!;
      const input = target.querySelector<HTMLElement>('[data-port="incoming"]')!;
      const arrow = Array.from(container.querySelectorAll(".stage-arrow"))
        .find((arrow) => arrow.getAttribute("data-from") === edge.from && arrow.getAttribute("data-to") === edge.to)!;
      const path = arrow.getAttribute("d")!;
      const start = /^M ([\d.]+) ([\d.]+)/.exec(path)!;
      const end = /L ([\d.]+) ([\d.]+)$/.exec(path)!;
      expect(Number.parseFloat(source.style.left) + Number.parseFloat(output.style.left)).toBeCloseTo(Number(start[1]));
      expect(Number.parseFloat(source.style.top) + Number.parseFloat(output.style.top)).toBeCloseTo(Number(start[2]));
      expect(Number.parseFloat(target.style.left) + Number.parseFloat(input.style.left)).toBeCloseTo(Number(end[1]));
      expect(Number.parseFloat(target.style.top) + Number.parseFloat(input.style.top)).toBeCloseTo(Number(end[2]));
    }
  });

  it("switches to top-to-bottom below 700px without shrinking cards", () => {
    let width = 900;
    vi.stubGlobal("ResizeObserver", undefined);
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => width);
    const { container } = render(<StageFlow stages={stages} variant="detail" />);
    const canvas = container.querySelector(".graph-canvas");
    expect(canvas).toHaveAttribute("data-direction", "LR");
    width = 699;
    fireEvent.resize(window);
    expect(canvas).toHaveAttribute("data-direction", "TB");
    for (const element of container.querySelectorAll(".stage-step")) {
      expect(element).toHaveStyle({ width: "210px" });
    }
    width = 700;
    fireEvent.resize(window);
    expect(canvas).toHaveAttribute("data-direction", "LR");
  });

  it.each<StageGraphVariant>(["overview", "detail"])("fits all four %s cards in the native window's graph panel", (variant) => {
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(1102);
    const configured = [
      stage("test"), stage("beta", ["test"]), stage("gamma", ["beta"]), stage("prod", ["gamma"]),
    ];
    const { container } = render(<StageFlow stages={configured} variant={variant} onSelectStage={vi.fn()} />);
    const canvas = container.querySelector<HTMLElement>(".graph-canvas")!;
    expect(canvas).toHaveAttribute("data-direction", "LR");
    expect(Number.parseFloat(canvas.style.width)).toBeLessThanOrEqual(1102);
    for (const configuredStage of configured) {
      const element = stageElement(container, configuredStage.id);
      expect(element).toHaveStyle({ width: "210px", height: variant === "detail" ? "156px" : "142px" });
      expect(Number.parseFloat(element.style.left) + Number.parseFloat(element.style.width)).toBeLessThan(1102);
      expect(screen.getByRole("button", { name: `${configuredStage.id}: not run` })).toBeVisible();
    }
  });

  it("preserves the complete text and grows the card for long stage IDs", () => {
    const longId = "a-very-long-stage-identifier-".repeat(4);
    const { container } = render(<StageFlow stages={[stage("start"), stage(longId, ["start"])]} onSelectStage={vi.fn()} />);
    expect(screen.getByRole("button", { name: `${longId}: not run` })).toBeVisible();
    expect(screen.getByText(longId)).toBeVisible();
    expect(Number.parseFloat(stageElement(container, longId).style.height))
      .toBeGreaterThan(Number.parseFloat(stageElement(container, "start").style.height));
  });

  it("reserves footer lines for planned policy labels and longer preview metadata", () => {
    const { container, rerender } = render(<StageFlow stages={stages} />);
    expect(Number.parseFloat(stageElement(container, "prod").style.height)).toBeGreaterThan(142);
    rerender(<StageFlow
      stages={[stage("test")]}
      preview={{ stages: { test: telemetry({ detail: "manual approval from release owner", metric: "evidence is pending" }) }, transitions: [] }}
    />);
    expect(Number.parseFloat(stageElement(container, "test").style.height)).toBeGreaterThan(142);
    expect(screen.getByText("manual approval from release owner")).toBeVisible();
    expect(screen.getByText("evidence is pending")).toBeVisible();
  });
});

describe("explicit stage preview", () => {
  it("renders the supplied six states in the reusable three-zone cards", () => {
    const tones: StageTone[] = ["idle", "done", "running", "waiting", "failed", "live"];
    const statuses = ["locked", "passed", "running", "gate", "failed", "live"];
    const configured = tones.map((tone) => stage(tone));
    const preview: StageFlowPreview = {
      stages: Object.fromEntries(tones.map((tone, index) => [tone, telemetry({
        tone,
        status: statuses[index]!,
        duration: `${index}:24`,
        summary: `${index} sample checks`,
        progress: tone === "idle" ? null : index * 20,
      })])),
      transitions: [],
    };
    const { container } = render(<StageFlow stages={configured} preview={preview} onSelectStage={vi.fn()} />);
    for (const [index, tone] of tones.entries()) {
      const button = screen.getByRole("button", { name: `${tone}: ${statuses[index]}` });
      expect(button).toHaveClass("stage-node", tone);
      expect(within(button).getByText(`${index}:24`)).toBeVisible();
      expect(within(button).getByText(`${index} sample checks`)).toBeVisible();
      expect(within(button).getByText("sample only")).toBeVisible();
      expect(within(button).getByText("simulated")).toBeVisible();
    }
    expect(container.querySelector(".stage-node.running .spinner-arc")).not.toBeNull();
    expect(container.querySelector(".stage-node.idle .status-icon rect")).not.toBeNull();
    expect(screen.getByRole("progressbar", { name: "running execution progress" })).toHaveAttribute("aria-valuenow", "40");
    expect(screen.queryByRole("progressbar", { name: "idle execution progress" })).not.toBeInTheDocument();
  });

  describe("stage consumers", () => {
    it("describes manual destinations without adding them to the executable graph", () => {
      const configured = [
        stage("test"),
        { ...stage("beta", ["test"]), consumers: [consumerFixture] },
        stage("prod", ["beta"]),
      ];
      const { container } = render(<StageFlow stages={configured} onSelectStage={vi.fn()} />);
      expect(screen.getByRole("button", { name: "beta: not run" })).toHaveAccessibleDescription(
        "After all of: test. Consumer destinations: Personal MacBook. Manual approval required.",
      );
      expect(screen.getByText("1 consumer / manual")).toBeVisible();
      expect(container.querySelectorAll(".stage-step")).toHaveLength(3);
      expect(container.querySelectorAll(".stage-arrow")).toHaveLength(2);
      expect(container.querySelector('[data-stage-id="personal-mac"]')).toBeNull();
      expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    });
  });

  it("makes only supplied promoting/next transitions keyboard-actionable", async () => {
    const onTransition = vi.fn();
    const user = userEvent.setup();
    const preview: StageFlowPreview = {
      stages: {},
      transitions: [
        { from: "test", to: "linux", tone: "done", label: "passed" },
        { from: "test", to: "macos", tone: "idle", label: "blocked" },
        { from: "linux", to: "prod", tone: "promoting", label: "promote" },
        { from: "macos", to: "prod", tone: "next", label: "next" },
        { from: "test", to: "prod", tone: "next", label: "invented edge" },
      ],
      onTransition,
    };
    const { container, rerender } = render(<StageFlow stages={stages} preview={preview} />);
    expect(screen.getAllByRole("button")).toHaveLength(2);
    const promote = screen.getByRole("button", { name: "linux to prod: promote (preview)" });
    promote.focus();
    await user.keyboard("{Enter}");
    expect(onTransition).toHaveBeenLastCalledWith("linux", "prod");
    screen.getByRole("button", { name: "macos to prod: next (preview)" }).focus();
    await user.keyboard(" ");
    expect(onTransition).toHaveBeenLastCalledWith("macos", "prod");
    expect(onTransition).toHaveBeenCalledTimes(2);
    expect(screen.queryByText("invented edge")).not.toBeInTheDocument();
    expect(container.querySelectorAll(".stage-arrow")).toHaveLength(4);
    rerender(<StageFlow stages={stages} preview={{ ...preview, onTransition: undefined }} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText("promote")).toBeVisible();
    expect(screen.getByText("next")).toBeVisible();
  });

  it("animates droplets on the exact routed fork/join paths, not a fixed horizontal shortcut", () => {
    const graph = layoutStages(stages);
    const preview: StageFlowPreview = {
      stages: {},
      transitions: graph.edges.map((edge) => ({ from: edge.from, to: edge.to, tone: "promoting", label: "promote" })),
    };
    const { container } = render(<StageFlow stages={stages} preview={preview} />);
    expect(container.querySelectorAll(".pipe-droplet")).toHaveLength(graph.edges.length * 3);
    for (const arrow of container.querySelectorAll(".stage-arrow")) {
      const path = arrow.getAttribute("d");
      expect(path).toContain(" Q ");
      for (const droplet of arrow.parentElement!.querySelectorAll<SVGElement>(".pipe-droplet")) {
        expect(droplet.style.offsetPath).toBe(`path("${path}")`);
        expect(droplet).toHaveAttribute("cx", "0");
        expect(droplet).toHaveAttribute("cy", "0");
      }
      expect(arrow).toHaveAttribute("marker-end", expect.stringContaining("-promoting)"));
    }
  });

  it("leaves omitted stage and transition telemetry unrun without inheriting success", () => {
    const configured = [stage("test"), stage("constructor", ["test"]), stage("prod", ["constructor"])];
    const preview: StageFlowPreview = {
      stages: { test: telemetry({ tone: "done", status: "passed", progress: 100 }) },
      transitions: [],
      onTransition: vi.fn(),
    };
    const { container } = render(<StageFlow stages={configured} preview={preview} />);
    expect(screen.getByRole("group", { name: "test: passed" })).toHaveClass("done");
    expect(screen.getByRole("group", { name: "constructor: not run" })).toHaveClass("idle");
    expect(screen.getByRole("group", { name: "prod: not run" })).toHaveClass("idle");
    expect(container.querySelector(".stage-node.idle .status-icon rect")).toBeNull();
    expect(container.querySelectorAll("g.transition.idle")).toHaveLength(2);
    expect(container.querySelector(".pipe-droplet")).toBeNull();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("keeps transition identities distinct when stage IDs contain delimiters", () => {
    const configured = [stage("a:b"), stage("c", ["a:b"]), stage("a"), stage("b:c", ["a"])];
    const preview: StageFlowPreview = {
      stages: {},
      transitions: [
        { from: "a:b", to: "c", tone: "next", label: "first" },
        { from: "a", to: "b:c", tone: "done", label: "second" },
      ],
      onTransition: vi.fn(),
    };
    render(<StageFlow stages={configured} preview={preview} />);
    expect(screen.getByRole("button", { name: "a:b to c: first (preview)" })).toBeVisible();
    expect(screen.getByText("second")).toBeVisible();
    expect(screen.getAllByRole("button")).toHaveLength(1);
  });

  it.each([
    [-10, "0"],
    [150, "100"],
    [Number.NaN, null],
    [Number.POSITIVE_INFINITY, null],
    [null, null],
  ])("handles a preview progress value of %s without invalid meter values", (progress, expected) => {
    const { container } = render(<StageFlow
      stages={[stage("test")]}
      preview={{ stages: { test: telemetry({ progress }) }, transitions: [] }}
    />);
    if (expected === null) expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    else expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", expected);
    expect(container.querySelector(".stage-meter-fill")).toHaveStyle({ width: `${expected ?? 0}%` });
  });

  it("disables motion and moving droplets for reduced-motion users while keeping status elements", () => {
    const filename = fileURLToPath(import.meta.url).replace(/\.test\.tsx$/, ".css");
    const stageFlowStyles = readFileSync(filename, "utf8");
    expect(stageFlowStyles).toMatch(/@media\s*\(prefers-reduced-motion:\s*reduce\)/);
    const reducedMotion = stageFlowStyles.slice(stageFlowStyles.lastIndexOf("@media"));
    expect(reducedMotion).toContain(".graph-frame *");
    expect(reducedMotion).toContain("animation: none !important");
    expect(reducedMotion).toContain("transition: none !important");
    expect(reducedMotion).toMatch(/\.pipe-droplet\s*\{\s*display:\s*none/);
    expect(reducedMotion).not.toMatch(/stage-(badge|status|meter|node)/);
  });

  it("anchors center-origin droplets to the SVG origin instead of the view-box midpoint", () => {
    const filename = fileURLToPath(import.meta.url).replace(/\.test\.tsx$/, ".css");
    const rules = /\.pipe-droplet\s*\{([^}]+)\}/.exec(readFileSync(filename, "utf8"))?.[1];
    expect(rules).toContain("transform-box: view-box");
    expect(rules).toContain("offset-anchor: 0 0");
  });
});
