import type { DisplayError } from "../lib/api";

export function ErrorBanner({ error }: { error: DisplayError }) {
  return (
    <div className="error-banner" role="alert">
      <strong>{error.code.replaceAll("_", " ")}</strong>
      <span>{error.message}</span>
    </div>
  );
}
