import type { CommandSummary } from "../lib/types";

function largeInteger(value: string | number | boolean): boolean {
  return typeof value === "number" && Number.isInteger(value) && !Number.isSafeInteger(value);
}

function statusValue(value: string | number | boolean): string {
  return largeInteger(value) ? "large integer / review original YAML" : JSON.stringify(value);
}

export function CommandResultContract({ command }: { command: CommandSummary }) {
  const result = command.result;
  return result?.type === "jsonFile" ? (
    <div className="command-result-contract">
      <span>result: JSON file <code>{result.path}</code> / pointer <code>{JSON.stringify(result.pointer)}</code>{!result.pointer && " (root)"}</span>
      <span>success: <code>{statusValue(result.success)}</code> / failure: <code>{statusValue(result.failure)}</code> (exact typed values)</span>
      {(largeInteger(result.success) || largeInteger(result.failure)) && <span className="compact-note">This status exceeds the browser's exact integer range. Review its original YAML value, or use a string status for lossless display. The backend compares the original numeric value.</span>}
      <span className="muted">Read after normal exit using an RFC 6901 pointer. Relative paths resolve from command cwd; absolute paths are preserved. The JSON value determines success regardless of exit code; signals fail. A fresh regular file up to 1 MiB is required. Missing, stale, invalid, or unknown results fail. Stdout is not parsed and no test counts are inferred.</span>
    </div>
  ) : (
    <span className="command-result-contract">result: exit code / success: {(result?.successCodes ?? [0]).join(", ")}{!result && " (default)"} / all other codes fail</span>
  );
}

export function CommandList({ commands, sourcePath }: { commands: CommandSummary[]; sourcePath?: string }) {
  return (
    <ul className="command-list">
      {commands.map((command) => (
        <li key={command.id}>
          <strong>{command.id}</strong>
          <code>{JSON.stringify(command.exec)}</code>
          <span className="muted">
            cwd: {command.cwd ?? sourcePath ?? "source root"}
            {command.cwd && sourcePath && ` / source: ${sourcePath}`}
            {command.timeout && ` / timeout: ${command.timeout}`}
          </span>
          <CommandResultContract command={command} />
          {Object.keys(command.env).length > 0 && (
            <details>
              <summary>environment</summary>
              <pre>{JSON.stringify(command.env, null, 2)}</pre>
            </details>
          )}
          {Object.keys(command.secrets).length > 0 && (
            <details>
              <summary>credential references</summary>
              <pre>{JSON.stringify(command.secrets, null, 2)}</pre>
            </details>
          )}
        </li>
      ))}
    </ul>
  );
}
