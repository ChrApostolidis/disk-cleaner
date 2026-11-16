import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { FileEntry, FileSort } from "../api/types";
import { formatBytes, formatDate, relativeFolder } from "../format";
import { canReveal, reveal } from "../reveal";

interface Props {
  files: FileEntry[];
  root: string;
  sort: FileSort;
  descending: boolean;
  onSort: (s: FileSort) => void;
  selection: Map<string, number>;
  /** Sets every given file to selected or not selected. */
  onSetMany: (items: { path: string; bytes: number }[], on: boolean) => void;
}

const COLUMNS: { key: FileSort | null; label: string; right?: boolean }[] = [
  { key: "name", label: "Name" },
  { key: null, label: "Folder" },
  { key: "allocated", label: "Size on disk", right: true },
  { key: "size", label: "Size", right: true },
  { key: "modified", label: "Modified", right: true },
];

const ROW_HEIGHT = 38;

export function FilesTab({ files, root, sort, descending, onSort, selection, onSetMany }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // Anchor of a shift-click range, by path so it survives re-sorting and refetching.
  const anchor = useRef<string | null>(null);

  const click = (index: number, shift: boolean) => {
    const f = files[index];
    const turnOn = !selection.has(f.path);
    const from = shift && anchor.current !== null ? files.findIndex((x) => x.path === anchor.current) : -1;
    const range = from === -1 ? [f] : files.slice(Math.min(from, index), Math.max(from, index) + 1);
    onSetMany(range.map((x) => ({ path: x.path, bytes: x.allocated })), turnOn);
    anchor.current = f.path;
  };
  const virtualizer = useVirtualizer({
    count: files.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  return (
    <div className="table" ref={scrollRef} role="table" aria-rowcount={files.length}>
      <div className="trow thead" role="row">
        <div className="th" />
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
          const f = files[v.index];
          return (
            <div
              key={f.path}
              className="trow"
              role="row"
              style={{ height: ROW_HEIGHT, transform: `translateY(${v.start}px)` }}
            >
              <div className="td">
                <input
                  type="checkbox"
                  checked={selection.has(f.path)}
                  aria-label={`Select ${f.name}`}
                  onChange={() => undefined}
                  onClick={(e) => click(v.index, e.shiftKey)}
                />
              </div>
              <div className="td ellipsis" title={f.name}>{f.name}</div>
              <div className="td ellipsis muted" title={f.path}>{relativeFolder(f.path, f.name, root)}</div>
              <div className="td right num">{formatBytes(f.allocated)}</div>
              <div className="td right num muted">{formatBytes(f.size)}</div>
              <div className="td right num muted">{formatDate(f.modified)}</div>
              <div className="td right">
                {canReveal && (
                  <button className="btn btn-small btn-ghost" onClick={() => reveal(f.path)}>
                    Show in Explorer
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
