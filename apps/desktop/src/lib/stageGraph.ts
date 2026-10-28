import { graphlib, layout, type EdgeLabel, type GraphLabel, type NodeLabel, type Point } from "@dagrejs/dagre";
import type { StageSummary } from "./types";

export type GraphDirection = "LR" | "TB";
export type StageGraphVariant = "overview" | "detail";

export interface StageNode {
  stage: StageSummary;
  left: number;
  top: number;
  width: number;
  height: number;
  ports: {
    incoming: Point | null;
    outgoing: Point | null;
  };
}

export interface StageEdge {
  from: string;
  to: string;
  points: Point[];
  path: string;
  labelPosition: Point;
}

export interface StageGraph {
  nodes: StageNode[];
  edges: StageEdge[];
  width: number;
  height: number;
}

class InvalidGraph extends Error {
  readonly code = "invalid_graph";
}

function coordinate(value: number | undefined, name: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new InvalidGraph(`The graph layout did not produce a valid ${name}.`);
  }
  return value;
}

function dockPoints(points: Point[], source: Point, target: Point, direction: GraphDirection): Point[] {
  const middle = points.length > 2 ? points.slice(1, -1) : [{
    x: (source.x + target.x) / 2,
    y: (source.y + target.y) / 2,
  }];
  const first = middle[0]!;
  const last = middle[middle.length - 1]!;
  // Leave each socket along the flow axis before joining Dagre's routing lanes.
  // This keeps forks and joins docked without cutting across neighboring cards.
  const docked = [
    source,
    direction === "LR" ? { x: first.x, y: source.y } : { x: source.x, y: first.y },
    ...middle,
    direction === "LR" ? { x: last.x, y: target.y } : { x: target.x, y: last.y },
    target,
  ];
  return docked.filter((point, index) => {
    const previous = docked[index - 1];
    return !previous || point.x !== previous.x || point.y !== previous.y;
  });
}

function roundedPath(points: Point[]): string {
  const format = (point: Point) => `${point.x.toFixed(2)} ${point.y.toFixed(2)}`;
  const first = points[0]!;
  const last = points[points.length - 1]!;
  let path = `M ${format(first)}`;
  for (let index = 1; index < points.length - 1; index += 1) {
    const previous = points[index - 1]!;
    const point = points[index]!;
    const next = points[index + 1]!;
    const beforeLength = Math.hypot(point.x - previous.x, point.y - previous.y);
    const afterLength = Math.hypot(next.x - point.x, next.y - point.y);
    const radius = Math.min(10, beforeLength / 2, afterLength / 2);
    const before = {
      x: point.x + (previous.x - point.x) * radius / beforeLength,
      y: point.y + (previous.y - point.y) * radius / beforeLength,
    };
    const after = {
      x: point.x + (next.x - point.x) * radius / afterLength,
      y: point.y + (next.y - point.y) * radius / afterLength,
    };
    path += ` L ${format(before)} Q ${format(point)} ${format(after)}`;
  }
  return `${path} L ${format(last)}`;
}

export function layoutStages(
  stages: readonly StageSummary[],
  direction: GraphDirection = "LR",
  variant: StageGraphVariant = "overview",
  measureFooterLines?: (stage: StageSummary) => number,
): StageGraph {
  if (stages.length === 0) {
    throw new InvalidGraph("The pipeline has no stages.");
  }

  const graph = new graphlib.Graph<GraphLabel, NodeLabel, EdgeLabel>({ directed: true });
  graph.setGraph({
    rankdir: direction,
    ranksep: direction === "LR" ? 4 : 56,
    nodesep: 56,
    edgesep: 24,
    marginx: 24,
    marginy: 24,
  });
  const card = variant === "detail"
    ? { width: 210, height: 156, charactersPerLine: 8, lineHeight: 28 }
    : { width: 210, height: 142, charactersPerLine: 10, lineHeight: 24 };
  const ids = new Map<string, string>();
  for (const [index, stage] of stages.entries()) {
    if (!stage.id || ids.has(stage.id)) {
      throw new InvalidGraph(`Stage IDs must be unique: "${stage.id}".`);
    }
    if (!Array.isArray(stage.after)) {
      throw new InvalidGraph(`Stage "${stage.id}" is missing its connections. Update and restart Pipes.`);
    }
    if (!Array.isArray(stage.consumers)) {
      throw new InvalidGraph(`Stage "${stage.id}" is missing consumer metadata. Update and restart Pipes.`);
    }
    if (!Array.isArray(stage.promotionCriteria)) {
      throw new InvalidGraph(`Stage "${stage.id}" is missing promotion criteria metadata. Update and restart Pipes.`);
    }
    // Dagre's traversal helpers use object keys; keep manifest IDs like
    // "constructor" out of that namespace without changing their displayed IDs.
    const key = `stage-${index}`;
    ids.set(stage.id, key);
    // Reserve title lines alongside the timer, in addition to all three card zones.
    const lines = Math.max(1, Math.ceil(stage.id.length / card.charactersPerLine));
    const footerLines = Math.max(1, Math.ceil(measureFooterLines?.(stage) ?? 1));
    graph.setNode(key, {
      width: card.width,
      height: card.height + (lines - 1) * card.lineHeight + (footerLines - 1) * 14
        + (stage.consumers.length > 0 ? 20 : 0) + (stage.promotionCriteria.length > 0 ? 20 : 0),
    });
  }

  for (const stage of stages) {
    const seen = new Set<string>();
    for (const parent of stage.after) {
      if (!ids.has(parent)) {
        throw new InvalidGraph(`Stage "${stage.id}" refers to missing stage "${parent}".`);
      }
      if (parent === stage.id || seen.has(parent)) {
        throw new InvalidGraph(`Stage "${stage.id}" has an invalid or repeated connection from "${parent}".`);
      }
      seen.add(parent);
      graph.setEdge(ids.get(parent)!, ids.get(stage.id)!, {
        width: 64,
        height: 24,
        labelpos: "r",
        labeloffset: 12,
      });
    }
  }
  if (!graphlib.alg.isAcyclic(graph)) {
    throw new InvalidGraph("The pipeline contains a cycle.");
  }

  layout(graph);

  const nodes = stages.map((stage): StageNode => {
    const key = ids.get(stage.id)!;
    const node = graph.node(key);
    if (!node) throw new InvalidGraph(`The layout lost stage "${stage.id}".`);
    const x = coordinate(node.x, "stage position");
    const y = coordinate(node.y, "stage position");
    return {
      stage,
      left: x - node.width / 2,
      top: y - node.height / 2,
      width: node.width,
      height: node.height,
      ports: {
        incoming: graph.inEdges(key)?.length
          ? direction === "LR" ? { x: x - node.width / 2, y } : { x, y: y - node.height / 2 }
          : null,
        outgoing: graph.outEdges(key)?.length
          ? direction === "LR" ? { x: x + node.width / 2, y } : { x, y: y + node.height / 2 }
          : null,
      },
    };
  });
  const nodesById = new Map(nodes.map((node) => [node.stage.id, node]));
  const edges = stages.flatMap((stage) => stage.after.map((parent): StageEdge => {
    const edge = graph.edge(ids.get(parent)!, ids.get(stage.id)!);
    const source = nodesById.get(parent)?.ports.outgoing;
    const target = nodesById.get(stage.id)?.ports.incoming;
    if (!edge?.points || edge.points.length < 2 || !source || !target) {
      throw new InvalidGraph(`The layout lost the connection "${parent}" to "${stage.id}".`);
    }
    const points = edge.points.map((point) => ({
      x: coordinate(point.x, "arrow position"),
      y: coordinate(point.y, "arrow position"),
    }));
    const routed = dockPoints(points, source, target, direction);
    return {
      from: parent,
      to: stage.id,
      points: routed,
      path: roundedPath(routed),
      labelPosition: {
        x: coordinate(edge.x, "connection label position"),
        y: coordinate(edge.y, "connection label position"),
      },
    };
  }));

  return {
    nodes,
    edges,
    width: coordinate(graph.graph().width, "graph width"),
    height: coordinate(graph.graph().height, "graph height"),
  };
}
