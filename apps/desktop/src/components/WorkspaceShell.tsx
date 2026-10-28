import type { ReactNode } from "react";

export type WorkspaceView = "pipelines" | "releases" | "manifest" | "storage";

const navigation: { view: WorkspaceView; label: string; name: string }[] = [
  { view: "pipelines", label: "Pipelines", name: "pipelines" },
  { view: "releases", label: "Releases", name: "releases" },
  { view: "manifest", label: "Manifest library", name: "manifest" },
  { view: "storage", label: "Local store", name: "storage" },
];

export function WorkspaceShell({
  view,
  onNavigate,
  breadcrumb,
  connection,
  topbar,
  banner,
  children,
}: {
  view: WorkspaceView;
  onNavigate(view: WorkspaceView): void;
  breadcrumb: string;
  connection: { label: string; tone: "connected" | "unavailable" | "preview"; host?: string; version?: string };
  topbar: ReactNode;
  banner?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="shell">
      <aside className="sidebar" aria-label="Workspace navigation">
        <button type="button" className="brand" onClick={() => onNavigate("pipelines")} aria-label="Pipes overview">
          p<span className="brand-word">ipes</span><span className="brand-cursor">_</span>
        </button>
        <p className="sidebar-label">LOCAL / CI</p>
        <nav className="navigation" aria-label="Primary">
          {navigation.map((item, index) => (
            <button
              type="button"
              key={item.view}
              className={`nav-item ${view === item.view ? "active" : ""}`}
              aria-current={view === item.view ? "page" : undefined}
              aria-label={item.label}
              title={item.label}
              onClick={() => onNavigate(item.view)}
            >
              <span className="nav-index">{String(index + 1).padStart(2, "0")}</span>
              <span className="nav-name">{item.name}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-foot">
          <span className={`connection ${connection.tone}`}>{connection.label}</span>
          <p>{connection.host ?? "local workspace"}<br />{connection.version ?? "native runtime required"}</p>
        </div>
      </aside>
      <main className="main">
        <header className="topbar">
          <span className="breadcrumb">workspace / <strong>{breadcrumb}</strong></span>
          <div className="topbar-right">{topbar}</div>
        </header>
        {banner}
        <div className="content">{children}</div>
      </main>
    </div>
  );
}
