import { useCallback, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { api } from "../api";
import type { RecycleBinItem, RecycleBinSummary } from "../api/types";
import { formatBytes, formatCount, formatDateTime, formatRelative, parentFolder } from "../format";
import { Modal } from "./Modal";

interface Props {
  bin: RecycleBinSummary | null;
  onBinChange: (s: RecycleBinSummary) => void;
  onRefreshBin: () => void;
  onClose: () => void;
}

type Step = "list" | "confirm" | "emptying" | "done";

const ROW_HEIGHT = 40;

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

const plural = (n: number) => (n === 1 ? "item" : "items");

export function RecycleBinDialog({ bin, onBinChange, onRefreshBin, onClose }: Props) {
  const [step, setStep] = useState<Step>("list");
  const [items, setItems] = useState<RecycleBinItem[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [emptyError, setEmptyError] = useState<string | null>(null);
  const [freed, setFreed] = useState(0);
  // Frozen when the confirmation opens so the wording matches what the user agreed to.
  const [confirmed, setConfirmed] = useState<RecycleBinSummary | null>(null);
  const alive = useRef(true);

  const load = useCallback(() => {
    setItems(null);
    setLoadError(null);
    api
      .listRecycleBin()
      .then((r) => {
        if (alive.current) setItems(r);
      })
      .catch((e: unknown) => {
        if (alive.current) setLoadError(message(e));
      });
  }, []);

  useEffect(() => {
    alive.current = true;
    load();
    onRefreshBin();
    return () => {
      alive.current = false;
    };
  }, [load, onRefreshBin]);

  const askConfirm = () => {
    setEmptyError(null);
    setConfirmed(bin ?? { items: items?.length ?? 0, size: 0 });
    setStep("confirm");
  };

  const empty = () => {
    const before = confirmed?.size ?? 0;
    setStep("emptying");
    api
      .emptyRecycleBin()
      .then((s) => {
        if (!alive.current) return;
        onBinChange(s);
        setFreed(before);
        setStep("done");
        load();
      })
      .catch((e: unknown) => {
        if (!alive.current) return;
        setEmptyError(message(e));
        setStep("list");
        onRefreshBin();
      });
  };

  const busy = step === "emptying";
  const hasItems = (bin?.items ?? items?.length ?? 0) > 0;

  return (
    <Modal
      title="Recycle Bin"
      labelId="recycle-bin-title"
      phase={step}
      wide
      // Esc backs out of the confirmation first; nothing closes while the bin is being emptied.
      onClose={busy ? null : step === "confirm" ? () => setStep("list") : onClose}
    >
      <div className="modal-body">
        <p className="small muted rb-sub">
          {bin ? (
            <>
              <span className="num">{formatCount(bin.items)}</span> {plural(bin.items)} ·{" "}
              <span className="num">{formatBytes(bin.size)}</span>
            </>
          ) : (
            "Reading the Recycle Bin…"
          )}
        </p>
        {step === "done" && (
          <p className="ok-text" role="status">
            Recycle Bin emptied, freed <strong className="num">{formatBytes(freed)}</strong>.
          </p>
        )}
        {emptyError && (
          <p className="err-text small" role="alert">
            Could not empty the Recycle Bin: {emptyError}
          </p>
        )}
        <BinList items={items} error={loadError} />
      </div>

      <div className="modal-foot">
        {step === "confirm" && confirmed ? (
          <>
            <p className="rb-confirm" role="alert">
              Permanently delete <span className="num">{formatCount(confirmed.items)}</span> {plural(confirmed.items)} (
              <span className="num">{formatBytes(confirmed.size)}</span>) from the Recycle Bin on all drives? This can't be
              undone. Items cleaned with DevDisk can no longer be put back.
            </p>
            <div className="modal-foot-row">
              <span />
              <span className="modal-foot-actions">
                <button className="btn" data-initial-focus onClick={() => setStep("list")}>
                  Cancel
                </button>
                <button className="btn btn-danger" onClick={empty}>
                  Empty permanently
                </button>
              </span>
            </div>
          </>
        ) : step === "emptying" ? (
          <div className="rb-busy" role="status">
            <span className="spinner" aria-hidden="true" />
            Emptying…
          </div>
        ) : (
          <div className="modal-foot-row">
            <span />
            <span className="modal-foot-actions">
              <button className="btn" data-initial-focus onClick={onClose}>
                Close
              </button>
              <button className="btn btn-danger" disabled={!hasItems} onClick={askConfirm}>
                Empty Recycle Bin…
              </button>
            </span>
          </div>
        )}
      </div>
    </Modal>
  );
}

function BinList({ items, error }: { items: RecycleBinItem[] | null; error: string | null }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const rows = items ?? [];
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });

  if (error) {
    return (
      <p className="err-text small" role="alert">
        Could not list the Recycle Bin: {error}
      </p>
    );
  }
  if (!items) {
    return (
      <p className="muted rb-loading" role="status">
        <span className="spinner" aria-hidden="true" />
        Measuring folders…
      </p>
    );
  }
  if (items.length === 0) return <p className="muted rb-loading">The Recycle Bin is empty.</p>;

  return (
    <div className="rb-table">
      <div className="rb-head rb-grid" aria-hidden="true">
        <span>Name</span>
        <span>Original folder</span>
        <span className="rb-num">Size</span>
        <span>Deleted</span>
      </div>
      <div className="rb-scroll" ref={scrollRef} tabIndex={0} role="table" aria-label="Recycle Bin contents" aria-rowcount={items.length}>
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((v) => {
            const it = rows[v.index];
            const folder = parentFolder(it.originalPath, it.name);
            return (
              <div
                key={it.id}
                role="row"
                className="rb-row rb-grid"
                style={{ height: ROW_HEIGHT, transform: `translateY(${v.start}px)` }}
              >
                <span className="rb-name" role="cell" title={it.name}>
                  <span className="ellipsis">{it.name}</span>
                  {it.fromDevDisk && <span className="tag">DevDisk</span>}
                </span>
                <span className="ellipsis muted small" role="cell" title={folder}>
                  {folder}
                </span>
                <span className="rb-num num" role="cell">
                  {it.size === null ? "—" : formatBytes(it.size)}
                </span>
                <span className="small muted" role="cell" title={formatDateTime(it.deletedAt)}>
                  {formatRelative(it.deletedAt)}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
