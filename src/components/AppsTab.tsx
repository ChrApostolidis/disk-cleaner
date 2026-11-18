import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { api } from "../api";
import type { AppKind, InstalledApp } from "../api/types";
import { formatBytes, formatDate } from "../format";
import { canReveal, reveal } from "../reveal";

type AppSort = "name" | "size" | "installed";
type KindFilter = "all" | AppKind;

const COLUMNS: { key: AppSort | null; label: string; right?: boolean }[] = [
  { key: "name", label: "Name" },
  { key: null, label: "Publisher" },
  { key: null, label: "Version" },
  { key: "installed", label: "Installed", right: true },
  { key: "size", label: "Size", right: true },
];

const KINDS: { key: KindFilter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "desktop", label: "Desktop" },
  { key: "store", label: "Store" },
];

const ROW_HEIGHT = 38;

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

/** Unknown values always sort last, whichever the direction. */
function compare(a: InstalledApp, b: InstalledApp, sort: AppSort, descending: boolean): number {
  const byName = a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  if (sort === "name") return descending ? -byName : byName;
  const va = sort === "size" ? a.size : a.installedAt;
  const vb = sort === "size" ? b.size : b.installedAt;
  if (va === null || vb === null) return (va === null ? 1 : 0) - (vb === null ? 1 : 0) || byName;
  return (descending ? vb - va : va - vb) || byName;
}

export function AppsTab() {
  const [apps, setApps] = useState<InstalledApp[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [kind, setKind] = useState<KindFilter>("all");
  const [sort, setSort] = useState<AppSort>("size");
  const [descending, setDescending] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    api
      .listApps()
      .then((r) => live && setApps(r))
      .catch((e: unknown) => live && setError(message(e)));
    return () => {
      live = false;
    };
  }, []);

  const shown = useMemo(() => {
    const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
    return (apps ?? [])
      .filter((a) => kind === "all" || a.kind === kind)
      .filter((a) => {
        const text = `${a.name} ${a.publisher ?? ""}`.toLowerCase();
        return words.every((w) => text.includes(w));
      })
      .sort((a, b) => compare(a, b, sort, descending));
  }, [apps, filter, kind, sort, descending]);

  const counts = useMemo(() => {
    const all = apps ?? [];
    return { all: all.length, desktop: all.filter((a) => a.kind === "desktop").length, store: all.filter((a) => a.kind === "store").length };
  }, [apps]);
  const knownTotal = shown.reduce((sum, a) => sum + (a.size ?? 0), 0);
  const unknown = shown.filter((a) => a.size === null).length;

  const virtualizer = useVirtualizer({
    count: shown.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  const onSort = (key: AppSort) => {
    if (key === sort) setDescending((d) => !d);
    else {
      setSort(key);
      setDescending(key !== "name");
    }
  };

  if (error) {
    return (
      <div className="banner banner-error" role="alert">
        Could not list installed apps: {error}
      </div>
    );
  }
  if (!apps) return <p className="muted">Reading installed apps…</p>;

  return (
    <div className="apps">
      <div className="apps-bar">
        <input
          className="search-input"
          type="search"
          placeholder="Filter by name or publisher"
          aria-label="Filter apps"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <div className="chips" role="group" aria-label="App type">
          {KINDS.map((k) => (
            <button
              key={k.key}
              className={`chip${kind === k.key ? " chip-on" : ""}`}
              aria-pressed={kind === k.key}
              onClick={() => setKind(k.key)}
            >
              {k.label} <span className="num">{counts[k.key]}</span>
            </button>
          ))}
        </div>
      </div>
      <p className="small muted apps-note">
        {shown.length} {shown.length === 1 ? "app" : "apps"} · <span className="num">{formatBytes(knownTotal)}</span>
        {unknown > 0 && <> · {unknown} without a known size</>}. Sizes marked <span className="tag">est.</span> are the
        installer's own estimate; the rest are measured from your scan.
      </p>

      {shown.length === 0 ? (
        <div className="empty">
          <h2>No apps match</h2>
          <p className="muted">Try another word or switch the type back to All.</p>
        </div>
      ) : (
        <div className="table" ref={scrollRef} role="table" aria-rowcount={shown.length}>
          <div className="trow apps-row thead" role="row">
            {COLUMNS.map((c) => {
              const cls = `th${c.right ? " right" : ""}`;
              if (!c.key) return <div key={c.label} className={cls} role="columnheader">{c.label}</div>;
              const key = c.key;
              const active = sort === key;
              return (
                <button
                  key={c.label}
                  className={`${cls} th-sortable${active ? " th-active" : ""}`}
                  role="columnheader"
                  aria-sort={active ? (descending ? "descending" : "ascending") : "none"}
                  onClick={() => onSort(key)}
                >
                  {c.label}
                  <span className="sort-mark" aria-hidden>
                    {active ? (descending ? "▾" : "▴") : ""}
                  </span>
                </button>
              );
            })}
            <div className="th" />
          </div>
          <div className="tbody" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((v) => {
              const a = shown[v.index];
              return (
                <div
                  key={a.id}
                  className="trow apps-row"
                  role="row"
                  style={{ height: ROW_HEIGHT, transform: `translateY(${v.start}px)` }}
                >
                  <div className="td app-name" title={a.location ?? a.name}>
                    <span className="ellipsis">{a.name}</span>
                    {a.kind === "store" && <span className="tag">Store</span>}
                  </div>
                  <div className="td ellipsis muted" title={a.publisher ?? undefined}>{a.publisher ?? "—"}</div>
                  <div className="td ellipsis muted num" title={a.version ?? undefined}>{a.version ?? "—"}</div>
                  <div className="td right num muted">{a.installedAt === null ? "—" : formatDate(a.installedAt)}</div>
                  <div
                    className="td right num"
                    title={
                      a.sizeSource === "scan"
                        ? "Measured from the scan"
                        : a.sizeSource === "windows"
                          ? "The installer's own estimate"
                          : "Not in the scan, and the installer did not record a size"
                    }
                  >
                    {a.size === null ? (
                      <span className="muted">—</span>
                    ) : (
                      <>
                        {a.sizeSource === "windows" && <span className="tag">est.</span>} {formatBytes(a.size)}
                      </>
                    )}
                  </div>
                  <div className="td right">
                    {canReveal && a.location && (
                      <button className="btn btn-small btn-ghost" onClick={() => reveal(a.location ?? "")}>
                        Show in Explorer
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
