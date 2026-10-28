import { useEffect, useId, useRef, useState } from "react";
import { OUTPUT_CHARACTER_LIMIT, OUTPUT_LINE_LIMIT, type OutputSnapshot } from "../lib/runOutput";
import type { PipelineRun } from "../lib/types";
import { isRunActive } from "../lib/pipelineRuns";

export function RunTerminal({
  run,
  output,
  cursor,
  stageFilter,
  onStageFilter,
}: {
  run: PipelineRun;
  output: OutputSnapshot;
  cursor: number;
  stageFilter: string;
  onStageFilter(value: string): void;
}) {
  const id = useId();
  const viewport = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);
  const [commandFilter, setCommandFilter] = useState("");
  const [streamFilter, setStreamFilter] = useState("");
  const keyFor = (stageId: string | null, commandId: string | null) => JSON.stringify([stageId, commandId]);
  const commands = run.commands.filter((command) => !stageFilter || command.stageId === (stageFilter === "__prepare" ? null : stageFilter));
  const activeCommand = commands.some((command) => keyFor(command.stageId, command.commandId) === commandFilter) ? commandFilter : "";
  const lines = output.lines.filter((line) =>
    (!stageFilter || line.stageId === (stageFilter === "__prepare" ? null : stageFilter)) &&
    (!activeCommand || keyFor(line.stageId, line.commandId) === activeCommand) &&
    (!streamFilter || line.stream === streamFilter));
  const filtered = Boolean(stageFilter || activeCommand || streamFilter);
  const active = isRunActive(run.status);

  useEffect(() => {
    const element = viewport.current;
    if (element && follow) element.scrollTop = element.scrollHeight;
  }, [output, follow, stageFilter, activeCommand, streamFilter]);

  return (
    <section className="run-terminal" aria-label="Pipeline terminal output">
      <header className="run-terminal-heading">
        <h2>output <span>/ {active ? "live" : "saved"}</span></h2>
        <label className="follow-toggle"><input type="checkbox" checked={follow} onChange={(event) => setFollow(event.currentTarget.checked)} />auto-follow {follow ? "on" : "off"}</label>
      </header>
      <div className="run-output-filters">
        <label htmlFor={`${id}-stage`}>Stage
          <select id={`${id}-stage`} value={stageFilter} onChange={(event) => { onStageFilter(event.currentTarget.value); setCommandFilter(""); }}>
            <option value="">all stages</option>
            <option value="__prepare">prepare / runner</option>
            {run.manifest.stages.map((stage) => <option key={stage.id} value={stage.id}>{stage.id}</option>)}
          </select>
        </label>
        <label htmlFor={`${id}-command`}>Command
          <select id={`${id}-command`} value={activeCommand} onChange={(event) => setCommandFilter(event.currentTarget.value)}>
            <option value="">all commands</option>
            {commands.map((command) => <option key={keyFor(command.stageId, command.commandId)} value={keyFor(command.stageId, command.commandId)}>{command.stageId ?? "prepare"} / {command.commandId}</option>)}
          </select>
        </label>
        <label htmlFor={`${id}-stream`}>Stream
          <select id={`${id}-stream`} value={streamFilter} onChange={(event) => setStreamFilter(event.currentTarget.value)}>
            <option value="">all streams</option><option value="stdout">stdout</option><option value="stderr">stderr</option><option value="system">system</option>
          </select>
        </label>
        <span className="run-cursor">cursor {cursor}</span>
      </div>
      {(output.discardedLines > 0 || output.discardedCharacters > 0) && (
        <p className="output-truncated" role="status">
          Showing a bounded tail of the saved log. {output.discardedLines.toLocaleString()} lines / {output.discardedCharacters.toLocaleString()} characters omitted from this view.
        </p>
      )}
      <div className="run-output-viewport" ref={viewport} role="region" aria-label="Command output text" tabIndex={0} aria-live="off" onScroll={() => {
        const element = viewport.current;
        if (element && follow && element.scrollHeight - element.scrollTop - element.clientHeight > 48) setFollow(false);
      }}>
        {lines.length ? lines.map((line) => (
          <div className={`output-line output-${line.stream}`} key={line.id} data-complete={line.complete ? "true" : "false"}>
            <span className="output-attribution" title={`${line.recordedAt} / sequence ${line.sequence}`}>
              <time dateTime={line.recordedAt}>{line.recordedAt.slice(11, 19)}</time>{" "}
              [{line.stageId ?? (line.commandId ? "prepare" : "runner")}{line.commandId ? `/${line.commandId}` : ""}] {line.stream}
            </span>
            <pre className="output-text">{line.text || "\u00a0"}</pre>
          </div>
        )) : (
          <p className="output-empty">{filtered ? "No output matches these filters." : active
            ? run.status === "waiting_approval" ? "Waiting for approval. No command output yet." : "Waiting for output. Commands may buffer until they flush."
            : "No output was recorded for this run."}</p>
        )}
      </div>
      <p className="run-output-note">Read-only stdout / stderr / system output. ANSI styling and control sequences are removed; carriage-return progress updates replace the current line. No shell input or PTY. Output depends on the program flushing. Tail limit: {OUTPUT_LINE_LIMIT.toLocaleString()} lines / {OUTPUT_CHARACTER_LIMIT.toLocaleString()} characters. Persisted output is capped at 64 MiB; retention truncation is reported as system output.</p>
    </section>
  );
}
