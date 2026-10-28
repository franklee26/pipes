import type { ReactNode } from "react";

export interface Tab<T extends string> {
  id: T;
  label: ReactNode;
}

export function TabBar<T extends string>({
  id,
  label,
  tabs,
  value,
  onChange,
}: {
  id: string;
  label: string;
  tabs: readonly Tab<T>[];
  value: T;
  onChange(value: T): void;
}) {
  return (
    <div className="detail-tabs" role="tablist" aria-label={label}>
      {tabs.map((tab, index) => (
        <button
          type="button"
          key={tab.id}
          role="tab"
          id={`${id}-tab-${tab.id}`}
          aria-selected={value === tab.id}
          aria-controls={`${id}-panel-${tab.id}`}
          tabIndex={value === tab.id ? 0 : -1}
          onClick={() => onChange(tab.id)}
          onKeyDown={(event) => {
            const nextIndex = event.key === "Home" ? 0
              : event.key === "End" ? tabs.length - 1
              : event.key === "ArrowRight" ? (index + 1) % tabs.length
              : event.key === "ArrowLeft" ? (index - 1 + tabs.length) % tabs.length
              : undefined;
            if (nextIndex === undefined) return;
            event.preventDefault();
            const next = tabs[nextIndex];
            if (next) {
              onChange(next.id);
              document.getElementById(`${id}-tab-${next.id}`)?.focus();
            }
          }}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
