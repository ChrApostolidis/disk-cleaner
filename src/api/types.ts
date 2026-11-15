// Contract between the React UI and the Rust backend (src-tauri).
// The Rust DTOs serialize to exactly these shapes (serde camelCase).
// Change this file and the Rust side together.

/** Unix time in milliseconds. */
export type UnixMs = number;

/** Payload of the `scan-progress` event. Sent at most every ~100 ms. */
export interface ScanProgress {
  files: number;
  dirs: number;
  /** Size on disk found so far, bytes. */
  bytes: number;
  currentPath: string;
}

/** Payload of the `scan-done` event, and the result of `get_summary`. */
export interface ScanSummary {
  root: string;
  files: number;
  dirs: number;
  /** Size on disk, bytes. */
  allocated: number;
  /** Logical size, bytes. */
  size: number;
  elapsedMs: number;
  issues: number;
  projects: number;
  /** Sum of every project's reclaimable size on disk, bytes. */
  reclaimable: number;
}

/** Payload of the `scan-error` event. */
export interface ScanError {
  message: string;
  cancelled: boolean;
}

export interface RegenerableFolder {
  /** Folder name, e.g. "node_modules". */
  name: string;
  path: string;
  /** Size on disk, bytes. */
  allocated: number;
}

export interface Project {
  /** Stable for one scan; index into the scan's project list. */
  id: number;
  path: string;
  /** Folder name, e.g. "my-shop". */
  name: string;
  /** e.g. "React project: my-shop". */
  displayName: string;
  /** Framework label or ecosystem name, e.g. "Next.js", "Rust". */
  label: string;
  framework: string | null;
  version: string | null;
  /** Ecosystem ids, e.g. ["node"], ["node", "php"]. */
  ecosystems: string[];
  packageManager: string | null;
  hasLockfile: boolean;
  reinstallCommand: string | null;
  regenerable: RegenerableFolder[];
  /** Size on disk of the whole project folder, bytes. */
  totalAllocated: number;
  /** Size on disk of its regenerable folders, bytes. */
  reclaimable: number;
  lastActivity: UnixMs | null;
  /** id of the enclosing project (monorepo root, Tauri app), or null. */
  parentId: number | null;
}

export interface FileEntry {
  path: string;
  name: string;
  /** Logical size, bytes. */
  size: number;
  /** Size on disk, bytes. */
  allocated: number;
  modified: UnixMs | null;
}

export type FileSort = "allocated" | "size" | "modified" | "name";

/**
 * Search bar syntax (parsed in Rust, devdisk-core search module):
 *   words           AND terms, case-insensitive substring of the name
 *   *.mp3  a?c      wildcard terms, must match the whole name
 *   "two words"     one term containing a space
 *   >500MB  <10KB   size on disk filters (B, KB, MB, GB, TB; 1024-based)
 *   older:90d       not modified in the last 90 days (m, h, d, w, y)
 *   newer:1h        modified in the last hour
 * An invalid filter makes the command reject with a readable message.
 */
export type SearchQuery = string;

/** Tauri command names and their arguments/results. All commands reject with a string message. */
export interface DevDiskApi {
  /** `start_scan`: starts a background scan; rejects if one is running or the path is not a folder. */
  startScan(root: string): Promise<void>;
  /** `cancel_scan`: no-op when nothing is running. */
  cancelScan(): Promise<void>;
  /** `get_summary`: null before the first finished scan. */
  getSummary(): Promise<ScanSummary | null>;
  /** `list_projects`: matching projects, biggest reclaimable first. Empty before a scan. */
  listProjects(query: SearchQuery): Promise<Project[]>;
  /** `list_files`: the top `limit` matching files in the given order. Empty before a scan. */
  listFiles(query: SearchQuery, sort: FileSort, descending: boolean, limit: number): Promise<FileEntry[]>;
  /** Event listeners. Each returns an unsubscribe function. */
  onProgress(cb: (p: ScanProgress) => void): Promise<() => void>;
  onDone(cb: (s: ScanSummary) => void): Promise<() => void>;
  onError(cb: (e: ScanError) => void): Promise<() => void>;
}

/** Tauri event names. */
export const EVENTS = {
  progress: "scan-progress",
  done: "scan-done",
  error: "scan-error",
} as const;

/** Tauri command names. Arguments are passed as camelCase objects, e.g. invoke("list_files", { query, sort, descending, limit }). */
export const COMMANDS = {
  startScan: "start_scan",
  cancelScan: "cancel_scan",
  getSummary: "get_summary",
  listProjects: "list_projects",
  listFiles: "list_files",
} as const;
