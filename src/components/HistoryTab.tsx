import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { HistoryEntry, RestoreResult } from "../api/types";
import { formatBytes, formatDateTime } from "../format";

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

interface Batch {
  id: string;
  at: number;
  entries: HistoryEntry[];
}

export function HistoryTab() {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Latest outcome per entry id; kept apart from `entries` so a reload does not erase error messages.
  const [results, setResults] = useState<Record<number, RestoreResult>>({});
  const [busy, setBusy] = useState<Set<number>>(new Set());
  const [restoredAny, setRestoredAny] = useState(false);

  const load = useCallback(() => {
    api
      .listHistory()
      .then((r) => {
        setEntries(r);
        setError(null);
      })
      .catch((e: unknown) => setError(message(e)));
  }, []);

  useEffect(load, [load]);

  const batches = useMemo<Batch[]>(() => {
    const map = new Map<string, Batch>();
    for (const e of entries ?? []) {
      const b = map.get(e.batchId);
      if (b) b.entries.push(e);
      else map.set(e.batchId, { id: e.batchId, at: e.deletedAt, entries: [e] });
    }
    return [...map.values()];
  }, [entries]);

  const restore = (ids: number[]) => {
    if (ids.length === 0) return;
    setBusy((b) => new Set([...b, ...ids]));
    api
      .restoreEntries(ids)
      .then((rs) => {
        setResults((prev) => ({ ...prev, ...Object.fromEntries(rs.map((r) => [r.id, r])) }));
        if (rs.some((r) => r.ok)) setRestoredAny(true);
        load();
      })
      .catch((e: unknown) => {
        const msg = message(e);
        setResults((prev) => ({
          ...prev,
          ...Object.fromEntries(ids.map((id) => [id, { id, ok: false, error: msg }])),
        }));
      })
      .finally(() =>
        setBusy((b) => {
          const next = new Set(b);
          ids.forEach((id) => next.delete(id));
          return next;
        }),
      );
  };

  if (error && !entries) {
    return (
      <div className="banner banner-error" role="alert">
        Could not load history: {error}
      </div>
    );
  }
  if (!entries) return <p className="muted">Loading history…</p>;
  if (entries.length === 0) {
    return (
      <div className="empty">
        <h2>Nothing cleaned yet</h2>
        <p className="muted">Items you move to the Recycle Bin with DevDisk show up here, so you can put them back.</p>
      </div>
    );
  }

  return (
    <div className="history">
      {restoredAny && (
        <div className="banner banner-info" role="status">
          Rescan to see restored items in the results.
        </div>
      )}
      {batches.map((b) => {
        const total = b.entries.reduce((a, e) => a + e.allocated, 0);
        const pending = b.entries.filter((e) => e.restoredAt === null && !busy.has(e.id));
        return (
          <section className="batch" key={b.id} aria-label={`Cleanup on ${formatDateTime(b.at)}`}>
            <header className="batch-head">
              <div>
                <span className="batch-date">{formatDateTime(b.at)}</span>
                <span className="muted small">
                  {" "}
                  · {b.entries.length} {b.entries.length === 1 ? "item" : "items"} ·{" "}
                  <span className="num">{formatBytes(total)}</span>
                </span>
              </div>
              <button className="btn btn-small" disabled={pending.length === 0} onClick={() => restore(pending.map((e) => e.id))}>
                Put back all
              </button>
            </header>
            <ul className="hlist">
              {b.entries.map((e) => {
                const r = results[e.id];
                const inBin = e.restoredAt === null;
                return (
                  <li className="hitem" key={e.id}>
                    <div className="hitem-main">
                      <div className="hitem-path ellipsis" title={e.path}>
                        {e.path}
                      </div>
                      <div className="small muted">
                        <span className="num">{formatBytes(e.allocated)}</span>
                        {e.projectLabel && <> · {e.projectLabel}</>}
                      </div>
                    </div>
                    <div className="hitem-status small">
                      {inBin ? <span className="muted">In Recycle Bin</span> : <span className="ok-text">Put back {formatDateTime(e.restoredAt ?? 0)}</span>}
                      {r && !r.ok && <div className="err-text">{r.error ?? "Could not put back"}</div>}
                    </div>
                    <button
                      className="btn btn-small"
                      disabled={!inBin || busy.has(e.id)}
                      onClick={() => restore([e.id])}
                    >
                      {busy.has(e.id) ? "Putting back…" : "Put back"}
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>
        );
      })}
    </div>
  );
}
