import type { RefObject } from "react";
import { formatBytes } from "../format";

interface Props {
  count: number;
  bytes: number;
  drawerOpen: boolean;
  viewRef: RefObject<HTMLButtonElement | null>;
  onClear: () => void;
  onView: () => void;
  onReview: () => void;
}

export function SelectionBar({ count, bytes, drawerOpen, viewRef, onClear, onView, onReview }: Props) {
  return (
    <div className="selbar" role="region" aria-label="Selection">
      <span className="selbar-text">
        <strong className="num">{count}</strong> {count === 1 ? "item" : "items"} ·{" "}
        <strong className="num">{formatBytes(bytes)}</strong> selected
      </span>
      <span className="selbar-actions">
        <button className="btn" ref={viewRef} aria-expanded={drawerOpen} aria-controls="selection-drawer" onClick={onView}>
          View
        </button>
        <button className="btn" onClick={onClear}>
          Clear
        </button>
        <button className="btn btn-primary" onClick={onReview}>
          Review cleanup…
        </button>
      </span>
    </div>
  );
}
