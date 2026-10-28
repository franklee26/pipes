import { describe, expect, it } from "vitest";
import { layoutStages, type GraphDirection, type StageGraph, type StageGraphVariant, type StageNode } from "./stageGraph";
import type { StageSummary } from "./types";
import { consumerFixture, promotionRegisteredFixture } from "../test/fixtures";

function stage(id: string, after: string[] = []): StageSummary {
  return { id, after, checks: [], checkPlan: [], requires: [], requiresApproval: false, consumers: [], promotionCriteria: [] };
}

function node(graph: StageGraph, id: string): StageNode {
  const found = graph.nodes.find((item) => item.stage.id === id);
  if (!found) throw new Error(`Missing test node: ${id}`);
  return found;
}

function connections(graph: StageGraph) {
  return graph.edges.map((edge) => `${edge.from}->${edge.to}`).sort();
}

function verifyGeometry(graph: StageGraph, direction: GraphDirection, variant: StageGraphVariant = "overview") {
  for (const current of graph.nodes) {
    expect(current.left).toBeGreaterThanOrEqual(0);
    expect(current.top).toBeGreaterThanOrEqual(0);
    expect(current.left + current.width).toBeLessThanOrEqual(graph.width);
    expect(current.top + current.height).toBeLessThanOrEqual(graph.height);
    expect(current.width).toBe(210);
    expect(current.height).toBeGreaterThanOrEqual(variant === "detail" ? 156 : 142);
    expect(Boolean(current.ports.incoming)).toBe(graph.edges.some((edge) => edge.to === current.stage.id));
    expect(Boolean(current.ports.outgoing)).toBe(graph.edges.some((edge) => edge.from === current.stage.id));
    for (const other of graph.nodes) {
      if (current === other) continue;
      const overlapX = current.left < other.left + other.width && other.left < current.left + current.width;
      const overlapY = current.top < other.top + other.height && other.top < current.top + current.height;
      expect(overlapX && overlapY, `${current.stage.id} overlaps ${other.stage.id}`).toBe(false);
    }
  }
  for (const edge of graph.edges) {
    const source = node(graph, edge.from);
    const target = node(graph, edge.to);
    expect(edge.points.length).toBeGreaterThanOrEqual(2);
    expect(edge.points[0]).toEqual(source.ports.outgoing);
    expect(edge.points.at(-1)).toEqual(target.ports.incoming);
    if (direction === "LR") {
      expect(source.left + source.width).toBeLessThan(target.left);
      expect(source.ports.outgoing).toEqual({
        x: source.left + source.width,
        y: source.top + source.height / 2,
      });
      expect(target.ports.incoming).toEqual({ x: target.left, y: target.top + target.height / 2 });
      expect(edge.points[1]!.y).toBe(edge.points[0]!.y);
      expect(edge.points[1]!.x).toBeGreaterThan(edge.points[0]!.x);
      expect(edge.points.at(-2)!.y).toBe(edge.points.at(-1)!.y);
      expect(edge.points.at(-2)!.x).toBeLessThan(edge.points.at(-1)!.x);
    } else {
      expect(source.top + source.height).toBeLessThan(target.top);
      expect(source.ports.outgoing).toEqual({
        x: source.left + source.width / 2,
        y: source.top + source.height,
      });
      expect(target.ports.incoming).toEqual({ x: target.left + target.width / 2, y: target.top });
      expect(edge.points[1]!.x).toBe(edge.points[0]!.x);
      expect(edge.points[1]!.y).toBeGreaterThan(edge.points[0]!.y);
      expect(edge.points.at(-2)!.x).toBe(edge.points.at(-1)!.x);
      expect(edge.points.at(-2)!.y).toBeLessThan(edge.points.at(-1)!.y);
    }
    for (const point of edge.points) {
      expect(Number.isFinite(point.x) && Number.isFinite(point.y)).toBe(true);
      expect(point.x).toBeGreaterThanOrEqual(0);
      expect(point.y).toBeGreaterThanOrEqual(0);
      expect(point.x).toBeLessThanOrEqual(graph.width);
      expect(point.y).toBeLessThanOrEqual(graph.height);
    }
    expect(edge.path).toMatch(/^M /);
    expect(edge.path).not.toMatch(/NaN|undefined|Infinity/);
    const first = edge.points[0]!;
    const last = edge.points.at(-1)!;
    expect(edge.path.startsWith(`M ${first.x.toFixed(2)} ${first.y.toFixed(2)}`)).toBe(true);
    expect(edge.path.endsWith(`L ${last.x.toFixed(2)} ${last.y.toFixed(2)}`)).toBe(true);
    expect(edge.labelPosition.x - 32).toBeGreaterThanOrEqual(0);
    expect(edge.labelPosition.y - 12).toBeGreaterThanOrEqual(0);
    expect(edge.labelPosition.x + 32).toBeLessThanOrEqual(graph.width);
    expect(edge.labelPosition.y + 12).toBeLessThanOrEqual(graph.height);
    for (const current of graph.nodes) {
      const overlapX = edge.labelPosition.x - 32 < current.left + current.width
        && current.left < edge.labelPosition.x + 32;
      const overlapY = edge.labelPosition.y - 12 < current.top + current.height
        && current.top < edge.labelPosition.y + 12;
      expect(overlapX && overlapY, `${edge.from}->${edge.to} label overlaps ${current.stage.id}`).toBe(false);
    }
  }
}

const diamond = [
  stage("test"),
  stage("linux", ["test"]),
  stage("macos", ["test"]),
  stage("prod", ["linux", "macos"]),
];

describe("stage graph layout", () => {
  it("adds readable policy space without changing the configured graph", () => {
    const stages = promotionRegisteredFixture.manifest.stages;
    const graph = layoutStages(stages);
    const without = layoutStages(stages.map((item) => ({ ...item, promotionCriteria: [] })));
    expect(connections(graph)).toEqual(connections(without));
    expect(node(graph, "beta").height).toBe(node(without, "beta").height + 20);
    verifyGeometry(graph, "LR");
  });
  it.each<GraphDirection>(["LR", "TB"])("reserves consumer label space without changing stage connections in %s", (direction) => {
    const base = [stage("test"), stage("beta", ["test"]), stage("prod", ["beta"])];
    const plain = layoutStages(base, direction);
    const withConsumers = layoutStages(base.map((item) => item.id === "beta"
      ? { ...item, consumers: [consumerFixture] } : item), direction);
    expect(connections(withConsumers)).toEqual(connections(plain));
    expect(withConsumers.nodes).toHaveLength(3);
    expect(node(withConsumers, "beta").height).toBe(node(plain, "beta").height + 20);
    verifyGeometry(withConsumers, direction);
  });
  it.each<StageGraphVariant>(["overview", "detail"])("fits a four-stage %s pipeline within the default native window", (variant) => {
    const graph = layoutStages([
      stage("test"), stage("beta", ["test"]), stage("gamma", ["beta"]), stage("prod", ["gamma"]),
    ], "LR", variant);
    expect(connections(graph)).toEqual(["beta->gamma", "gamma->prod", "test->beta"]);
    expect(new Set(graph.nodes.map((item) => item.top)).size).toBe(1);
    expect(graph.width).toBe(1092);
    expect(graph.width).toBeLessThanOrEqual(1102);
    verifyGeometry(graph, "LR", variant);
  });

  it.each<GraphDirection>(["LR", "TB"])("lays out forks and all-parent joins in %s", (direction) => {
    const graph = layoutStages(diamond, direction);
    expect(connections(graph)).toEqual(["linux->prod", "macos->prod", "test->linux", "test->macos"]);
    expect(node(graph, "prod").stage.after).toEqual(["linux", "macos"]);
    if (direction === "LR") {
      expect(node(graph, "linux").left).toBe(node(graph, "macos").left);
      expect(node(graph, "linux").top).not.toBe(node(graph, "macos").top);
    } else {
      expect(node(graph, "linux").top).toBe(node(graph, "macos").top);
      expect(node(graph, "linux").left).not.toBe(node(graph, "macos").left);
    }
    verifyGeometry(graph, direction);
  });

  it("keeps branches independent when they never rejoin", () => {
    const graph = layoutStages([
      stage("test"), stage("desktop", ["test"]), stage("server", ["test"]),
      stage("desktop-prod", ["desktop"]), stage("server-prod", ["server"]),
    ]);
    expect(connections(graph)).toEqual([
      "desktop->desktop-prod", "server->server-prod", "test->desktop", "test->server",
    ]);
    verifyGeometry(graph, "LR");
  });

  it("supports a join alongside a branch that ends separately", () => {
    const graph = layoutStages([
      ...diamond,
      stage("documentation", ["test"]),
      stage("publish-docs", ["documentation"]),
    ]);
    expect(connections(graph)).toEqual([
      "documentation->publish-docs", "linux->prod", "macos->prod",
      "test->documentation", "test->linux", "test->macos",
    ]);
    verifyGeometry(graph, "LR");
  });

  it("uses topology, not declaration order, for forward references", () => {
    const stages = [diamond[3], diamond[1], diamond[2], diamond[0]].filter(
      (item): item is StageSummary => item !== undefined,
    );
    const graph = layoutStages(stages);
    expect(graph.nodes.map((item) => item.stage.id)).toEqual(["prod", "linux", "macos", "test"]);
    expect(node(graph, "test").left).toBeLessThan(node(graph, "prod").left);
    verifyGeometry(graph, "LR");
  });

  it("does not invent links between independent roots or leaves", () => {
    const graph = layoutStages([
      stage("one"), stage("two"), stage("one-end", ["one"]), stage("two-end", ["two"]), stage("standalone"),
    ]);
    expect(connections(graph)).toEqual(["one->one-end", "two->two-end"]);
    verifyGeometry(graph, "LR");
  });

  it("keeps manifest IDs distinct from inherited object properties", () => {
    const graph = layoutStages([
      stage("constructor"),
      stage("__proto__", ["constructor"]),
      stage("toString", ["__proto__"]),
    ]);
    expect(connections(graph)).toEqual(["__proto__->toString", "constructor->__proto__"]);
    expect(graph.nodes.map((node) => node.stage.id)).toEqual(["constructor", "__proto__", "toString"]);
    verifyGeometry(graph, "LR");
  });

  it("supports nested forks and joins without overlapping boxes", () => {
    const graph = layoutStages([
      stage("start"),
      stage("one", ["start"]),
      stage("two", ["start"]),
      stage("one-a", ["one"]),
      stage("one-b", ["one"]),
      stage("one-joined", ["one-a", "one-b"]),
      stage("all-joined", ["one-joined", "two"]),
      stage("end", ["all-joined"]),
    ]);
    expect(graph.edges).toHaveLength(9);
    verifyGeometry(graph, "LR");
  });

  it("reserves readable label space instead of scaling long identifiers", () => {
    const longId = "long-stage-".repeat(5) + "end";
    const graph = layoutStages([stage("start"), stage(longId, ["start"]), stage("end", [longId])]);
    expect(node(graph, longId).height).toBeGreaterThan(node(graph, "start").height);
    expect(node(graph, longId).width).toBe(node(graph, "start").width);
    verifyGeometry(graph, "LR");
  });

  it.each<GraphDirection>(["LR", "TB"])("docks long, rank-skipping connections in %s", (direction) => {
    const longId = "build-for-an-unusually-long-stage-name";
    const graph = layoutStages([
      stage("start"),
      stage(longId, ["start"]),
      stage("quick", ["start"]),
      stage("deep", [longId]),
      stage("deeper", ["deep"]),
      stage("end", ["deeper", "quick", "start"]),
    ], direction);
    expect(connections(graph)).toContain("start->end");
    expect(graph.edges.find((edge) => edge.from === "start" && edge.to === "end")!.points.length)
      .toBeGreaterThan(5);
    verifyGeometry(graph, direction);
  });

  it.each<GraphDirection>(["LR", "TB"])("reserves larger detail typography and matching ports in %s", (direction) => {
    const graph = layoutStages([...diamond, stage("a-long-detail-stage-name", ["prod"])], direction, "detail");
    expect(node(graph, "a-long-detail-stage-name").height).toBeGreaterThan(156);
    verifyGeometry(graph, direction, "detail");
  });

  it("adds space for wrapped footer labels without widening compact cards", () => {
    const graph = layoutStages([stage("one"), stage("two", ["one"])], "LR", "overview", (stage) => (
      stage.id === "two" ? 3 : 1
    ));
    expect(node(graph, "one").height).toBe(142);
    expect(node(graph, "two").height).toBe(170);
    verifyGeometry(graph, "LR");
  });

  it.each<GraphDirection>(["LR", "TB"])("keeps large graphs readable for local scrolling in %s", (direction) => {
    const stages = Array.from({ length: 32 }, (_, index) => stage(
      `stage-${index}`,
      index === 0 ? [] : [`stage-${index - 1}`],
    ));
    const graph = layoutStages(stages, direction);
    expect(direction === "LR" ? graph.width : graph.height).toBeGreaterThan(6_000);
    expect(graph.edges).toHaveLength(31);
    verifyGeometry(graph, direction);
  });

  it("is deterministic and does not mutate the manifest", () => {
    const stages = structuredClone(diamond);
    for (const item of stages) {
      Object.freeze(item.after);
      Object.freeze(item);
    }
    Object.freeze(stages);
    expect(layoutStages(stages)).toEqual(layoutStages(stages));
    expect(stages).toEqual(diamond);
  });

  it.each([
    [stage("test"), stage("test")],
    [stage("test", ["missing"])],
    [stage("test", ["test"])],
    [stage("one", ["two"]), stage("two", ["one"])],
    [stage("start"), stage("end", ["start", "start"])],
    [],
  ])("rejects invalid graph input rather than drawing a misleading graph", (...stages) => {
    expect(() => layoutStages(stages)).toThrow();
  });

  it("reports missing predecessor metadata rather than guessing a linear path", () => {
    const legacy = stage("test");
    Reflect.deleteProperty(legacy, "after");
    expect(() => layoutStages([legacy])).toThrow("missing its connections");
  });
});
