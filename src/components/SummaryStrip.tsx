import { useEffect, useState } from "react";
import type { ScanSummary } from "../api/types";
import { formatBytes, formatCount, formatDateTime, formatDuration, formatRelative } from "../format";

const STALE_MS = 24 * 3_600_000;

export function SummaryStrip({ s }: { s: ScanSummary }) {
  // Re-render each minute so "12 min ago" keeps moving.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);
  const stale = now - s.scannedAt > STALE_MS;

  const stats: [string, string][] = [
    ["On disk", formatBytes(s.allocated)],
    ["Files", formatCount(s.files)],
    ["Folders", formatCount(s.dirs)],
    ["Projects", formatCount(s.projects)],
    ["Scan time", formatDuration(s.elapsedMs)],
    ["Issues", formatCount(s.issues)],
  ];
  return (
    <section className="summary" aria-label="Scan summary">
      <div className="summary-hero">
        <div className="stat-label">Reclaimable</div>
        <div className="hero-value num">{formatBytes(s.reclaimable)}</div>
      </div>
      {stats.map(([label, value]) => (
        <div className="stat" key={label}>
          <div className="stat-label">{label}</div>
          <div className="stat-value num">{value}</div>
        </div>
      ))}
      <div className="scan-age small muted">
        <span title={formatDateTime(s.scannedAt)}>Scanned {formatRelative(s.scannedAt, now)}</span>
        {stale && <span className="stale-hint">may be out of date</span>}
      </div>
    </section>
  );
}
