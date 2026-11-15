import { formatBytes, formatCount, truncateMiddle } from "../format";
import type { ScanProgress } from "../api/types";

interface Props {
  root: string;
  onRootChange: (v: string) => void;
  /** True when the OS folder dialog is available; otherwise a text input is shown. */
  hasDialog: boolean;
  onChoose: () => void;
  scanning: boolean;
  onScan: () => void;
  onCancel: () => void;
  progress: ScanProgress | null;
}

export function TopBar(p: Props) {
  return (
    <header className="topbar">
      <div className="topbar-row">
        <h1 className="brand">DevDisk</h1>
        {p.hasDialog ? (
          <div className="root-path" title={p.root}>
            {p.root || <span className="muted">No folder chosen</span>}
          </div>
        ) : (
          <input
            className="root-input"
            value={p.root}
            onChange={(e) => p.onRootChange(e.target.value)}
            aria-label="Folder to scan"
            spellCheck={false}
            disabled={p.scanning}
          />
        )}
        {p.hasDialog && (
          <button className="btn" onClick={p.onChoose} disabled={p.scanning}>
            Choose folder
          </button>
        )}
        {p.scanning ? (
          <button className="btn" onClick={p.onCancel}>
            Cancel
          </button>
        ) : (
          <button className="btn btn-primary" onClick={p.onScan} disabled={!p.root.trim()}>
            Scan
          </button>
        )}
      </div>
      {p.scanning && (
        <div className="progress" role="status">
          <div className="progress-bar" />
          <div className="progress-text">
            <span className="num">{formatCount(p.progress?.files ?? 0)} files</span>
            <span className="num">{formatCount(p.progress?.dirs ?? 0)} folders</span>
            <span className="num">{formatBytes(p.progress?.bytes ?? 0)}</span>
            <span className="progress-path muted" title={p.progress?.currentPath}>
              {truncateMiddle(p.progress?.currentPath ?? "", 80)}
            </span>
          </div>
        </div>
      )}
    </header>
  );
}
