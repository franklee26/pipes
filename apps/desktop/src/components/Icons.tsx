export function Arrow({ className = "" }: { className?: string }) {
  return (
    <svg className={`icon ${className}`} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 12h17m-6-6 6 6-6 6" />
    </svg>
  );
}

export function Plus() {
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 4v16M4 12h16" />
    </svg>
  );
}

export function Play() {
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      <path d="m8 4 12 8-12 8Z" />
    </svg>
  );
}

export function Device() {
  return (
    <svg className="icon" viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3" y="3" width="18" height="13" rx="1" />
      <path d="M8 21h8m-4-5v5" />
    </svg>
  );
}

export function StatusIcon({ tone, status }: {
  tone: "idle" | "done" | "running" | "waiting" | "failed" | "live";
  status?: string;
}) {
  return (
    <svg className={`icon status-icon status-${tone}`} viewBox="0 0 24 24" aria-hidden="true">
      {tone === "running" ? (
        <>
          <circle className="spinner-track" cx="12" cy="12" r="8" />
          <circle className="spinner-core" cx="12" cy="12" r="2" />
          <path className="spinner-arc" d="M12 4a8 8 0 0 1 8 8" />
        </>
      ) : tone === "done" || tone === "live" ? (
        <path d="m5 12 4 4L19 6" />
      ) : tone === "failed" ? (
        <path d="m5 5 14 14M19 5 5 19" />
      ) : tone === "waiting" ? (
        <path d="M12 4v10m0 4v2" />
      ) : status && /^(locked|blocked|retained)$/i.test(status) ? (
        <><rect x="6" y="10" width="12" height="10" rx="1" /><path d="M9 10V7a3 3 0 0 1 6 0v3m-3 4v3" /></>
      ) : (
        <><circle cx="12" cy="12" r="8" /><path d="M12 7v5l3 2" /></>
      )}
    </svg>
  );
}
