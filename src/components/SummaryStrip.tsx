import type { ScanSummary } from "../api/types";
import { formatBytes, formatCount, formatDuration } from "../format";

export function SummaryStrip({ s }: { s: ScanSummary }) {
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
    </section>
  );
}
