const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${UNITS[i]}`;
}

export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}

const DAY = 86_400_000;

export function formatAge(ms: number | null, now = Date.now()): string {
  if (ms === null) return "unknown";
  const days = Math.floor((now - ms) / DAY);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 60) return `${days} days ago`;
  if (days < 730) return `${Math.floor(days / 30)} months ago`;
  return `${Math.floor(days / 365)} years ago`;
}

export function formatDate(ms: number | null): string {
  if (ms === null) return "unknown";
  return new Date(ms).toLocaleDateString("en-GB", { year: "numeric", month: "short", day: "numeric" });
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`;
}

export function truncateMiddle(s: string, max = 70): string {
  if (s.length <= max) return s;
  const keep = max - 1;
  const head = Math.ceil(keep / 2);
  return `${s.slice(0, head)}…${s.slice(s.length - (keep - head))}`;
}

/** Path relative to the scan root, falling back to the full path. Handles both separators. */
export function relativePath(path: string, root: string): string {
  const norm = (p: string) => p.replace(/\//g, "\\");
  const p = norm(path);
  const r = norm(root).replace(/\\+$/, "");
  if (p.toLowerCase() === r.toLowerCase()) return ".";
  if (p.toLowerCase().startsWith(r.toLowerCase() + "\\")) return p.slice(r.length + 1);
  return p;
}

/** Folder part of a path (path minus the file name), relative to the root. */
export function relativeFolder(path: string, name: string, root: string): string {
  const full = path.endsWith(name) ? path.slice(0, path.length - name.length) : path;
  const rel = relativePath(full.replace(/[\\/]+$/, ""), root);
  return rel === "." ? "" : rel;
}

export function formatDateTime(ms: number): string {
  return new Date(ms).toLocaleString("en-GB", { year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export function baseName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
}

const MINUTE = 60_000;
const HOUR = 3_600_000;

/** "just now", "12 min ago", "3 h ago", "yesterday at 18:40", "3 days ago", then a plain date. */
export function formatRelative(ms: number, now = Date.now()): string {
  const diff = Math.max(0, now - ms);
  if (diff < MINUTE) return "just now";
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)} min ago`;
  const d = new Date(ms);
  const startOfToday = new Date(now).setHours(0, 0, 0, 0);
  if (ms >= startOfToday && diff < 24 * HOUR) return `${Math.floor(diff / HOUR)} h ago`;
  const time = d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  if (ms >= startOfToday - DAY && ms < startOfToday) return `yesterday at ${time}`;
  const days = Math.floor((startOfToday - new Date(ms).setHours(0, 0, 0, 0)) / DAY);
  if (days < 7) return `${days} days ago`;
  return formatDate(ms);
}

/** Folder part of an absolute path: the path minus its last segment. */
export function parentFolder(path: string, name: string): string {
  const trimmed = path.replace(/[\/]+$/, "");
  return trimmed.endsWith(name) ? trimmed.slice(0, trimmed.length - name.length).replace(/[\/]+$/, "") : trimmed;
}
