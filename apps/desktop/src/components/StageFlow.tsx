import { useEffect, useId, useMemo, useRef, useState } from "react";
import { displayError, type DisplayError } from "../lib/api";
import { layoutStages, type GraphDirection, type StageGraph, type StageGraphVariant } from "../lib/stageGraph";
import type { StageSummary } from "../lib/types";
import { ErrorBanner } from "./ErrorBanner";
import { Device, StatusIcon } from "./Icons";
import "./StageFlow.css";

export type StageTone = "idle" | "done" | "running" | "waiting" | "failed" | "live";

export interface StageTelemetry {
  tone: StageTone;
  role: string;
  status: string;
  duration: string;
  summary: string;
  progress: number | null;
  detail: string;
  metric: string;
}

export interface TransitionTelemetry {
  from: string;
  to: string;
  tone: "idle" | "done" | "promoting" | "next";
  label: string;
}

export interface StageFlowTelemetry {
  stages: Record<string, StageTelemetry>;
  transitions: TransitionTelemetry[];
}

export interface StageFlowPreview extends StageFlowTelemetry {
  onTransition?(from: string, to: string): void;
}

interface StageFlowProps {
  stages: StageSummary[];
  selectedStage?: string;
  onSelectStage?(id: string): void;
  label?: string;
  preview?: StageFlowPreview;
  telemetry?: StageFlowTelemetry;
  variant?: StageGraphVariant;
}

type LayoutResult =
  | { kind: "ready"; graph: StageGraph }
  | { kind: "error"; error: DisplayError };

function plannedTelemetry(stage: StageSummary): StageTelemetry {
  return {
    tone: "idle",
    role: stage.after.length === 0 ? "entry" : stage.after.length > 1 ? "join / all" : "stage",
    status: "not run",
    duration: "--:--",
    summary: `${stage.checks.length} ${stage.checks.length === 1 ? "check" : "checks"} planned`,
    progress: null,
    detail: stage.requiresApproval ? "manual approval" : "no approval gate",
    metric: stage.requires.length > 0
      ? `${stage.requires.length} ${stage.requires.length === 1 ? "rule" : "rules"} planned`
      : "planned",
  };
}

function transitionKey(from: string, to: string): string {
  return JSON.stringify([from, to]);
}

function stageTelemetry(stage: StageSummary, telemetry?: StageFlowTelemetry): StageTelemetry {
  return (telemetry && Object.hasOwn(telemetry.stages, stage.id) && telemetry.stages[stage.id])
    || plannedTelemetry(stage);
}

export function StageFlow({
  stages,
  selectedStage,
  onSelectStage,
  label = "Configured pipeline",
  preview,
  telemetry,
  variant = "overview",
}: StageFlowProps) {
  const frame = useRef<HTMLDivElement>(null);
  const display = telemetry ?? preview;
  const [direction, setDirection] = useState<GraphDirection>("LR");
  const id = useId().replaceAll(":", "");
  const markerId = (tone: TransitionTelemetry["tone"]) => `stage-arrow-${id}-${tone}`;

  useEffect(() => {
    const element = frame.current;
    if (!element) return;
    const measure = () => {
      if (element.clientWidth > 0) setDirection(element.clientWidth < 700 ? "TB" : "LR");
    };
    measure();
    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver(measure);
      observer.observe(element);
      return () => observer.disconnect();
    }
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);

  const result = useMemo<LayoutResult>(() => {
    try {
      const graph = layoutStages(stages, direction, variant, (stage) => {
        const telemetry = stageTelemetry(stage, display);
        const columns = variant === "detail" ? 28 : 29;
        return Math.ceil((telemetry.detail.length + telemetry.metric.length) / columns);
      });
      return { kind: "ready", graph };
    } catch (failure) {
      return { kind: "error", error: displayError(failure) };
    }
  }, [stages, direction, variant, display?.stages]);
  const transitionTelemetry = useMemo(() => new Map(
    display?.transitions.map((transition) => [transitionKey(transition.from, transition.to), transition]),
  ), [display?.transitions]);
  const conduits = result.kind === "ready" ? result.graph.edges.map((edge) => ({
    edge,
    telemetry: transitionTelemetry.get(transitionKey(edge.from, edge.to)) ?? {
      from: edge.from,
      to: edge.to,
      tone: "idle" as const,
      label: "planned",
    },
  })) : [];
  const arrowTones = [...new Set(conduits.map(({ telemetry }) => telemetry.tone))];

  return (
    <div className={`graph-frame graph-${variant}`} data-preview={preview && !telemetry ? "true" : undefined} ref={frame}>
      {result.kind === "error" ? <ErrorBanner error={result.error} /> : (
        <div className="graph-viewport" role="region" aria-label={`${label} graph`} tabIndex={0}>
          <div
            className="graph-canvas"
            data-direction={direction}
            style={{ width: result.graph.width, height: result.graph.height }}
          >
            <svg
              className="graph-edges"
              width={result.graph.width}
              height={result.graph.height}
              viewBox={`0 0 ${result.graph.width} ${result.graph.height}`}
              aria-hidden="true"
            >
              <defs>
                {arrowTones.map((tone) => (
                  <marker
                    key={tone}
                    id={markerId(tone)}
                    className={`transition ${tone}`}
                    markerWidth="14"
                    markerHeight="14"
                    refX="11"
                    refY="7"
                    orient="auto"
                    markerUnits="userSpaceOnUse"
                    viewBox="0 0 14 14"
                    overflow="visible"
                  >
                    <path className="pipe-head" d="M1 1 L11 7 L1 13" />
                  </marker>
                ))}
              </defs>
              {conduits.map(({ edge, telemetry }) => (
                <g className={`transition ${telemetry.tone}`} key={transitionKey(edge.from, edge.to)}>
                  <path className="pipe-track" d={edge.path} />
                  <path
                    className="stage-arrow pipe-base"
                    data-from={edge.from}
                    data-to={edge.to}
                    d={edge.path}
                    markerEnd={`url(#${markerId(telemetry.tone)})`}
                  />
                  <path className="pipe-stream" d={edge.path} />
                  {telemetry.tone !== "idle" && [0, 1, 2].map((droplet) => (
                    <circle
                      key={droplet}
                      className={`pipe-droplet delay-${droplet}`}
                      cx="0"
                      cy="0"
                      r="2.5"
                      style={{ offsetPath: `path("${edge.path}")` }}
                    />
                  ))}
                </g>
              ))}
            </svg>
            <div className="graph-transitions">
              {conduits.map(({ edge, telemetry }) => {
                const actionable = display === preview && preview?.onTransition && (telemetry.tone === "promoting" || telemetry.tone === "next");
                const className = `edge-label ${telemetry.tone}`;
                const style = { left: edge.labelPosition.x, top: edge.labelPosition.y };
                const title = `${edge.from} to ${edge.to}: ${telemetry.label}`;
                return actionable ? (
                  <button
                    key={transitionKey(edge.from, edge.to)}
                    type="button"
                    className={className}
                    style={style}
                    aria-label={`${title} (preview)`}
                    title={title}
                    onClick={() => preview?.onTransition?.(edge.from, edge.to)}
                  >
                    {telemetry.label}
                  </button>
                ) : (
                  <span
                    key={transitionKey(edge.from, edge.to)}
                    className={className}
                    style={style}
                    title={title}
                    aria-hidden="true"
                  >
                    {telemetry.label}
                  </span>
                );
              })}
            </div>
            <ol className="stage-flow" aria-label={label}>
              {result.graph.nodes.map((node, index) => {
                const stage = node.stage;
                const telemetry = stageTelemetry(stage, display);
                const progress = telemetry.progress !== null && Number.isFinite(telemetry.progress)
                  ? Math.max(0, Math.min(100, telemetry.progress))
                  : null;
                const descriptionId = `stage-description-${id}-${index}`;
                const className = `stage-node ${telemetry.tone}${selectedStage === stage.id ? " selected" : ""}`;
                const content = (
                  <>
                    <span className="stage-header">
                      <span className="stage-step-id">
                        <span className="stage-index">{String(index + 1).padStart(2, "0")}</span>
                        <span className="stage-role" title={telemetry.role}>{telemetry.role}</span>
                      </span>
                      <span className="stage-badge" title={telemetry.status}>
                        <StatusIcon tone={telemetry.tone} status={telemetry.status} />
                        <span className="stage-status">{telemetry.status}</span>
                      </span>
                    </span>
                    <span className="stage-body">
                      <span className="stage-title-row">
                        <strong className="stage-name">{stage.id}</strong>
                        <span className="stage-time" title={telemetry.duration}>{telemetry.duration}</span>
                      </span>
                      <span className="stage-result" title={telemetry.summary}>{telemetry.summary}</span>
                      {stage.consumers.length > 0 && (
                        <span className="stage-consumer-count" title={stage.consumers.map((consumer) => consumer.name).join(", ")}>
                          <Device />{stage.consumers.length} {stage.consumers.length === 1 ? "consumer" : "consumers"} / manual
                        </span>
                      )}
                      {stage.promotionCriteria.length > 0 && (
                        <span className="stage-policy-count">{stage.promotionCriteria.length} promotion {stage.promotionCriteria.length === 1 ? "rule" : "rules"}</span>
                      )}
                    </span>
                    <span className="stage-footer">
                      <span
                        className="stage-meter"
                        role={progress === null ? undefined : "progressbar"}
                        aria-label={progress === null ? undefined : `${stage.id} execution progress`}
                        aria-hidden={progress === null ? true : undefined}
                        aria-valuemin={progress === null ? undefined : 0}
                        aria-valuemax={progress === null ? undefined : 100}
                        aria-valuenow={progress ?? undefined}
                      >
                        <span className="stage-meter-fill" style={{ width: `${progress ?? 0}%` }} />
                      </span>
                      <span className="stage-submeta">
                        <span>{telemetry.detail}</span>
                        <span>{telemetry.metric}</span>
                      </span>
                    </span>
                  </>
                );
                return (
                  <li
                    className="stage-step"
                    key={stage.id}
                    data-stage-id={stage.id}
                    data-tone={telemetry.tone}
                    style={{ left: node.left, top: node.top, width: node.width, height: node.height }}
                  >
                    {onSelectStage ? (
                      <button
                        type="button"
                        className={className}
                        aria-label={`${stage.id}: ${telemetry.status}`}
                        aria-describedby={descriptionId}
                        aria-pressed={selectedStage === stage.id}
                        onClick={() => onSelectStage(stage.id)}
                      >
                        {content}
                      </button>
                    ) : (
                      <div
                        className={className}
                        role="group"
                        aria-label={`${stage.id}: ${telemetry.status}`}
                        aria-describedby={descriptionId}
                      >
                        {content}
                      </div>
                    )}
                    {(["incoming", "outgoing"] as const).map((kind) => {
                      const port = node.ports[kind];
                      return port && (
                        <span
                          key={kind}
                          className={`stage-port stage-port-${kind}`}
                          data-port={kind}
                          style={{ left: port.x - node.left, top: port.y - node.top }}
                          aria-hidden="true"
                        />
                      );
                    })}
                    <span className="sr-only" id={descriptionId}>
                      {stage.after.length > 0 ? `After all of: ${stage.after.join(", ")}.` : "Entry stage. No predecessors."}
                      {stage.consumers.length > 0 && ` Consumer destinations: ${stage.consumers.map((consumer) => consumer.name).join(", ")}. Manual approval required.`}
                    </span>
                  </li>
                );
              })}
            </ol>
          </div>
        </div>
      )}
    </div>
  );
}
