import { useCallback, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { api } from "../api";
import type { DupProgress, DuplicateGroup, Duplicates, FileEntry } from "../api/types";
import { formatBytes, formatCount, formatDate, formatRelative, relativePath } from "../format";
import { selectionState, type SelectedItem, type Selection } from "../selection";
import { canReveal, reveal } from "../reveal";

const LIMIT = 1000;
const HEAD_HEIGHT = 44;
const ROW_HEIGHT = 40;
const GAP = 12;

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

function item(f: FileEntry): SelectedItem {
  return {
    path: f.path,
    name: f.name,
    kind: "file",
    allocated: f.allocated,
    projectPath: null,
    projectLabel: null,
    source: "duplicates",
  };
}

interface Props {
  /** Debounced search bar text. */
  query: string;
  root: string;
  /** Bumped after a cleanup, so removed copies drop out. */
  dataVersion: number;
  selection: Selection;
  onSetMany: (items: SelectedItem[], on: boolean) => void;
  onSearchError: (error: string | null) => void;
}

function ProgressPanel({ progress, onCancel }: { progress: DupProgress | null; onCancel: () => void }) {
  const p = progress;
  const share = p && p.bytesTotal > 0 ? Math.min(1, p.bytesDone / p.bytesTotal) : 0;
  const what = !p
    ? "Grouping files by size…"
    : p.phase === "partial"
      ? `Comparing the first megabyte of ${formatCount(p.filesTotal)} files that share a size`
      : `Comparing ${formatCount(p.filesTotal)} whole files whose first megabyte matched`;
  return (
    <div className="progress dup-progress" role="status">
      <div className="dup-progress-head">
        <span>
          <span className="small muted">{p?.phase === "full" ? "Step 2 of 2" : "Step 1 of 2"} · </span>
          {what}
        </span>
        <button className="btn btn-small" onClick={onCancel}>
          Cancel
        </button>
      </div>
      <div className="meter" aria-hidden>
        <div className="meter-fill" style={{ width: `${(share * 100).toFixed(1)}%` }} />
      </div>
      {p && (
        <div className="progress-text small muted">
          <span className="num">
            {formatCount(p.filesDone)} of {formatCount(p.filesTotal)} files
          </span>
          <span className="num">
            {formatBytes(p.bytesDone)} of {formatBytes(p.bytesTotal)} read
          </span>
        </div>
      )}
    </div>
  );
}

function GroupCard({
  group,
  root,
  selection,
  onSetMany,
}: {
  group: DuplicateGroup;
  root: string;
  selection: Selection;
  onSetMany: (items: SelectedItem[], on: boolean) => void;
}) {
  const states = group.files.map((f) => selectionState(selection, f.path));
  const picked = states.filter((s) => s !== null).length;
  return (
    <section className="batch" aria-label={`${group.files.length} copies of ${group.files[0].name}`}>
      <header className="batch-head dup-head">
        <div className="ellipsis">
          <span className="batch-date">
            {group.files.length} copies · {formatBytes(group.size)} each
          </span>
          <span className="muted small"> · {group.files[0].name}</span>
        </div>
        <span className="small num reclaim-text" title="Size on disk freed by keeping one copy">
          {formatBytes(group.reclaimable)} extra
        </span>
      </header>
      <ul className="hlist">
        {group.files.map((f, i) => {
          const state = states[i];
          // The last unpicked copy can't be picked too: one copy always stays.
          const lastCopy = state === null && picked === group.files.length - 1;
          return (
            <li key={f.path} className="dup-row">
              <input
                type="checkbox"
                checked={state !== null}
                disabled={state === "covered" || lastCopy}
                title={
                  state === "covered"
                    ? "Included in a selected folder"
                    : lastCopy
                      ? "Keep at least one copy"
                      : undefined
                }
                aria-label={`Select ${f.path}`}
                onChange={() => onSetMany([item(f)], state === null)}
              />
              <div className="dup-file">
                <span className="ellipsis" title={f.path}>
                  {relativePath(f.path, root)}
                </span>
                {i === 0 && (
                  <span className="tag" title="Modified first, likely the original">
                    Oldest
                  </span>
                )}
              </div>
              <div className="right num muted small">{formatDate(f.modified)}</div>
              <div className="right">
                {canReveal && (
                  <button className="btn btn-small btn-ghost" onClick={() => reveal(f.path)}>
                    Show in Explorer
                  </button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

export function DuplicatesTab({ query, root, dataVersion, selection, onSetMany, onSearchError }: Props) {
  // undefined: first load still running; null: no search on this scan yet.
  const [result, setResult] = useState<Duplicates | null | undefined>(undefined);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<DupProgress | null>(null);
  const [error, setError] = useState<{ text: string; cancelled: boolean } | null>(null);
  const requestId = useRef(0);
  const scrollRef = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    const id = ++requestId.current;
    api
      .getDuplicates(query, LIMIT)
      .then((r) => {
        if (id !== requestId.current) return;
        setResult(r);
        onSearchError(null);
      })
      .catch((e: unknown) => {
        // A bad search keeps the last good list on screen, like the other tabs.
        if (id === requestId.current) onSearchError(message(e));
      });
  }, [query, onSearchError]);

  useEffect(() => {
    load();
  }, [load, dataVersion]);
  useEffect(() => () => onSearchError(null), [onSearchError]);

  useEffect(() => {
    let cancelled = false;
    const unsubs: Array<() => void> = [];
    const track = (p: Promise<() => void>) =>
      p.then((u) => {
        if (cancelled) u();
        else unsubs.push(u);
      });
    void track(api.onDuplicatesProgress((p) => setProgress(p)));
    void track(
      api.onDuplicatesDone(() => {
        setRunning(false);
        setProgress(null);
        load();
      }),
    );
    void track(
      api.onDuplicatesError((e) => {
        setRunning(false);
        setProgress(null);
        setError({ text: e.message, cancelled: e.cancelled });
      }),
    );
    return () => {
      cancelled = true;
      unsubs.forEach((u) => u());
    };
  }, [load]);

  const start = () => {
    setError(null);
    setProgress(null);
    setRunning(true);
    api.startDuplicates().catch((e: unknown) => {
      setRunning(false);
      setError({ text: message(e), cancelled: false });
    });
  };

  const groups = result?.groups ?? [];
  const virtualizer = useVirtualizer({
    count: groups.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => HEAD_HEIGHT + groups[i].files.length * ROW_HEIGHT + GAP,
    overscan: 6,
  });

  /** Picks every copy except one per group: the oldest one not picked already (files come oldest first). */
  const pickCopies = () => {
    const add: SelectedItem[] = [];
    for (const g of groups) {
      const free = g.files.filter((f) => selectionState(selection, f.path) === null);
      add.push(...free.slice(1).map(item));
    }
    if (add.length > 0) onSetMany(add, true);
  };

  const banner = error && (
    <div className={`banner ${error.cancelled ? "banner-info" : "banner-error"}`} role="alert">
      {error.cancelled ? "Duplicate search cancelled." : `Duplicate search failed: ${error.text}`}
    </div>
  );

  if (running) {
    return (
      <div className="dups">
        <ProgressPanel progress={progress} onCancel={() => void api.cancelDuplicates()} />
      </div>
    );
  }
  if (result === undefined) return null;
  if (result === null) {
    return (
      <div className="dups">
        {banner}
        <div className="empty">
          <h2>Find duplicate files</h2>
          <p className="muted">
            Compares the content of files that have the same size, so only real copies match, whatever their names.
            Every candidate file is read, which can take a few minutes on a large scan.
          </p>
          <p className="muted small">
            Left out: files under 1 MB, cloud-only and hard-linked files, and files inside .git, node_modules,
            build folders and system folders.
          </p>
          <button className="btn btn-primary" onClick={start}>
            Find duplicates
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="dups">
      {banner}
      <div className="apps-bar">
        <span>
          <span className="num">{formatCount(result.totalGroups)}</span>{" "}
          {result.totalGroups === 1 ? "file has" : "files have"} copies ·{" "}
          <span className="num reclaim-text">{formatBytes(result.totalReclaimable)}</span> in extra copies
        </span>
        <span className="small muted">Found {formatRelative(result.foundAt)}</span>
        <span className="dups-actions">
          <button
            className="btn btn-small"
            onClick={pickCopies}
            disabled={groups.length === 0}
            title="Selects every copy except the oldest in each group"
          >
            Select extra copies
          </button>
          <button className="btn btn-small btn-ghost" onClick={start}>
            Search again
          </button>
        </span>
      </div>
      {(result.issueCount > 0 || result.hardLinked > 0 || result.totalGroups > groups.length) && (
        <details className="small muted dups-notes">
          <summary>
            {[
              result.totalGroups > groups.length &&
                `Showing the ${formatCount(groups.length)} groups that free the most`,
              result.issueCount > 0 &&
                `${formatCount(result.issueCount)} ${result.issueCount === 1 ? "file" : "files"} could not be read`,
              result.hardLinked > 0 && `${formatCount(result.hardLinked)} hard-linked files left out`,
            ]
              .filter(Boolean)
              .join(" · ")}
          </summary>
          {result.hardLinked > 0 && (
            <p>Hard-linked files already share their data on disk, so removing one link frees nothing.</p>
          )}
          {result.issues.length > 0 && (
            <ul className="fail-list">
              {result.issues.map((i) => (
                <li key={i.path}>
                  <span>{i.path}</span>
                  <span className="err-text">{i.message}</span>
                </li>
              ))}
            </ul>
          )}
        </details>
      )}
      {groups.length === 0 ? (
        <div className="empty">
          <h2>{query.trim() ? "No duplicates match this search" : "No duplicates found"}</h2>
          <p className="muted">
            {query.trim()
              ? "A group shows up when any of its copies matches. Try fewer words or remove a filter."
              : "No two files of 1 MB or more have the same content in this scan."}
          </p>
        </div>
      ) : (
        <div className="dups-list" ref={scrollRef}>
          <div className="dups-inner" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((v) => (
              <div
                key={groups[v.index].id}
                className="dups-item"
                data-index={v.index}
                ref={virtualizer.measureElement}
                style={{ transform: `translateY(${v.start}px)` }}
              >
                <GroupCard group={groups[v.index]} root={root} selection={selection} onSetMany={onSetMany} />
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
