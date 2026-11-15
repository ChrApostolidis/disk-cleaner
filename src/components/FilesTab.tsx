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
}

const COLUMNS: { key: FileSort | null; label: string; right?: boolean }[] = [
  { key: "name", label: "Name" },
  { key: null, label: "Folder" },
  { key: "allocated", label: "Size on disk", right: true },
  { key: "size", label: "Size", right: true },
  { key: "modified", label: "Modified", right: true },
];

const ROW_HEIGHT = 38;

export function FilesTab({ files, root, sort, descending, onSort }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: files.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  return (
    <div className="table" ref={scrollRef} role="table" aria-rowcount={files.length}>
      <div className="trow thead" role="row">
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
