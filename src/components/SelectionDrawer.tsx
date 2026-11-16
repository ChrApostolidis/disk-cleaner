import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { formatBytes, relativePath } from "../format";
import type { SelectedItem, SelectionSource } from "../selection";

interface Props {
  items: SelectedItem[];
  root: string;
  onRemove: (path: string) => void;
  onClear: () => void;
  onReview: () => void;
  onClose: () => void;
}

const SOURCE_LABEL: Record<SelectionSource, string> = { projects: "Projects", files: "Files", map: "Map" };

interface Group {
  label: string | null;
  items: SelectedItem[];
  bytes: number;
}

function groupItems(items: SelectedItem[]): Group[] {
  const by = new Map<string | null, SelectedItem[]>();
  for (const it of items) {
    const list = by.get(it.projectLabel) ?? [];
    list.push(it);
    by.set(it.projectLabel, list);
  }
  const groups: Group[] = [...by].map(([label, list]) => ({
    label,
    items: [...list].sort((a, b) => b.allocated - a.allocated),
    bytes: list.reduce((a, it) => a + it.allocated, 0),
  }));
  // "Not in a project" always last.
  return groups.sort((a, b) => (a.label === null ? 1 : b.label === null ? -1 : a.label.localeCompare(b.label)));
}

export function SelectionDrawer({ items, root, onRemove, onClear, onReview, onClose }: Props) {
  const closeRef = useRef<HTMLButtonElement>(null);
  // Fixed below the top bar and above the selection bar, so neither is covered.
  const [inset, setInset] = useState({ top: 0, bottom: 0 });
  const groups = useMemo(() => groupItems(items), [items]);
  const bytes = items.reduce((a, it) => a + it.allocated, 0);

  useLayoutEffect(() => {
    const measure = () => {
      const top = document.querySelector(".topbar")?.getBoundingClientRect().bottom ?? 0;
      const bar = document.querySelector(".selbar")?.getBoundingClientRect().top;
      setInset({ top: top + 8, bottom: bar === undefined ? 8 : window.innerHeight - bar + 8 });
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // The review dialog handles its own Escape; don't close both at once.
      if (e.key === "Escape" && !document.querySelector(".modal-backdrop")) onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <aside
      id="selection-drawer"
      className="drawer"
      role="dialog"
      aria-modal="false"
      aria-label="Selected items"
      style={{ top: inset.top, bottom: inset.bottom }}
    >
      <header className="drawer-head">
        <div>
          <h2 className="drawer-title">Selected</h2>
          <div className="muted small num">
            {items.length} {items.length === 1 ? "item" : "items"} · {formatBytes(bytes)}
          </div>
        </div>
        <button className="btn btn-small" ref={closeRef} onClick={onClose}>
          Close
        </button>
      </header>
      <div className="drawer-body">
        {groups.map((g) => (
          <section className="dgroup" key={g.label ?? "__none"}>
            <div className="dgroup-head">
              <span className="dgroup-name">{g.label ?? "Not in a project"}</span>
              <span className="muted small num">{formatBytes(g.bytes)}</span>
            </div>
            <ul className="dlist">
              {g.items.map((it) => (
                <li className="ditem" key={it.path}>
                  <div className="ditem-main">
                    <div className="ditem-name" title={it.name}>{it.name}</div>
                    <div className="ditem-path muted small" title={it.path}>{relativePath(it.path, root)}</div>
                    <div className="small muted">{SOURCE_LABEL[it.source]}</div>
                  </div>
                  <span className="num ditem-size">{formatBytes(it.allocated)}</span>
                  <button
                    className="ditem-x"
                    aria-label={`Remove ${it.name} from selection`}
                    title="Remove"
                    onClick={() => onRemove(it.path)}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
      <footer className="drawer-foot">
        <button className="btn" onClick={onClear}>
          Clear all
        </button>
        <button className="btn btn-primary" onClick={onReview}>
          Review cleanup…
        </button>
      </footer>
    </aside>
  );
}
