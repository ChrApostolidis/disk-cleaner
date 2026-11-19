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

/**
 * Payload of the `scan-done` event, and the result of `get_summary`.
 * At startup the backend loads the last saved scan (if any) in the background and then emits `scan-done` with it,
 * so a UI that mounted earlier still receives it.
 */
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
  /** When this scan finished. A scan restored from disk at startup keeps its original time. */
  scannedAt: UnixMs;
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

/** The volume (drive) holding the scan root. */
export interface VolumeInfo {
  /** Volume root, e.g. "C:\\". */
  mount: string;
  /** Bytes. */
  total: number;
  /** Bytes free on the volume. */
  free: number;
}

/** "others" aggregates the children that did not make the per-folder cut. */
export type TreemapKind = "dir" | "file" | "others";

export interface TreemapNode {
  /** For "others": the parent folder's path. */
  path: string;
  /** For "others": e.g. "1,204 smaller items". */
  name: string;
  kind: TreemapKind;
  /** Size on disk, bytes. Block area is proportional to this. */
  allocated: number;
  fileCount: number;
  /** Label of the nearest enclosing project (this node included), e.g. "Next.js"; null outside projects. */
  label: string | null;
  /** True when this folder is itself a project root. */
  projectRoot: boolean;
  /** True when this folder is a regenerable folder (node_modules, target, ...) of its enclosing project. */
  regenerable: boolean;
  /**
   * Biggest first, at most 60 real entries plus one "others" entry; zero-size entries go into "others".
   * null for files, "others", and folders below the requested depth (load them with another get_treemap call).
   * [] for an empty folder.
   */
  children: TreemapNode[] | null;
}

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

/**
 * Cleanup checks, per item:
 *   "ok"     can go to the Recycle Bin
 *   "warn"   can go, but the preview must show the reasons (e.g. dependency folder without a lockfile)
 *   "refuse" will be skipped (protected path, git-tracked, too big for the Recycle Bin, Recycle Bin off, ...)
 */
export type CleanupVerdict = "ok" | "warn" | "refuse";

export interface CleanupPreviewItem {
  path: string;
  name: string;
  kind: "file" | "dir";
  /** Size on disk, bytes. */
  allocated: number;
  /** A regenerable folder (node_modules, target, ...) of its project. */
  regenerable: boolean;
  /** Enclosing project, if any. */
  projectPath: string | null;
  projectLabel: string | null;
  verdict: CleanupVerdict;
  /** Human-readable reasons for "warn"/"refuse"; empty for "ok". */
  reasons: string[];
}

export interface CleanupPreview {
  items: CleanupPreviewItem[];
  /** Sum of `allocated` over items that are not refused. */
  totalFreed: number;
}

/** Payload of `cleanup-progress`. */
export interface CleanupProgress {
  batchId: string;
  done: number;
  total: number;
  currentPath: string;
}

export interface CleanupResultItem {
  path: string;
  ok: boolean;
  /** Why it failed or was skipped; null when ok. */
  error: string | null;
  /** History entry id when ok. */
  entryId: number | null;
}

/** A project whose dependency folder was just removed and that has a known reinstall command. */
export interface ReinstallOffer {
  projectPath: string;
  /** e.g. "npm ci". */
  command: string;
}

/** Payload of `cleanup-done`. */
export interface CleanupDone {
  batchId: string;
  /** Bytes actually moved to the Recycle Bin. */
  freed: number;
  items: CleanupResultItem[];
  reinstall: ReinstallOffer[];
}

/** One item moved to the Recycle Bin by DevDisk. */
export interface HistoryEntry {
  id: number;
  /** All items of one cleanup share a batchId. */
  batchId: string;
  deletedAt: UnixMs;
  path: string;
  allocated: number;
  projectPath: string | null;
  projectLabel: string | null;
  /** Set once put back. */
  restoredAt: UnixMs | null;
}

export interface RestoreResult {
  id: number;
  ok: boolean;
  /** e.g. "no longer in the Recycle Bin", "a folder already exists at that path". */
  error: string | null;
}

/** Payload of `reinstall-output`: one line of the reinstall command's output. */
export interface ReinstallOutput {
  projectPath: string;
  stream: "stdout" | "stderr";
  line: string;
}

/** Payload of `reinstall-done`. */
export interface ReinstallDone {
  projectPath: string;
  success: boolean;
  exitCode: number | null;
}

/** The Windows Recycle Bin across all drives (not only DevDisk's items). */
export interface RecycleBinSummary {
  items: number;
  /** Bytes, as Windows reports for the whole bin. */
  size: number;
}

export interface RecycleBinItem {
  /** Opaque id for this item. */
  id: string;
  name: string;
  /** Where it was deleted from, e.g. "C:\Users\me\proj\node_modules". */
  originalPath: string;
  deletedAt: UnixMs;
  kind: "file" | "dir";
  /** Bytes; null when it could not be measured. */
  size: number | null;
  /** True when DevDisk's cleanup log has this item (same path, deleted at about the same time). */
  fromDevDisk: boolean;
}

export type AppKind = "desktop" | "store";

/** An installed app: a classic desktop program or a Microsoft Store app. */
export interface InstalledApp {
  /** Registry key or package full name; stable while the app stays installed. */
  id: string;
  name: string;
  publisher: string | null;
  version: string | null;
  kind: AppKind;
  /** Install folder, for "Show in Explorer". Sometimes guessed from the app's icon path. */
  location: string | null;
  /** Install day (midnight UTC); null when Windows does not record it (always for Store apps). */
  installedAt: UnixMs | null;
  /** Bytes; null when unknown. */
  size: number | null;
  /** "scan": measured from the current scan. "windows": the installer's own estimate. */
  sizeSource: "scan" | "windows" | null;
}

/** "scan": from the current scan. "measured": walked when listed. "windows": the installer's estimate. */
export type DevSizeSource = "scan" | "measured" | "windows";

/** One installed version of a language, or one part of a tool (the app, its data folders). */
export interface DevInstall {
  /** e.g. "v24.22.1", "stable-x86_64-pc-windows-msvc", "Extensions". */
  label: string;
  version: string | null;
  path: string | null;
  /** The version that runs when you type the command (first on PATH, rustup default, newest SDK). */
  active: boolean;
  /** Bytes; null when unknown. */
  size: number | null;
  sizeSource: DevSizeSource | null;
}

export interface DevItem {
  /** e.g. "node", ".net", "docker". */
  id: string;
  /** e.g. "Node.js", "Docker Desktop". */
  name: string;
  kind: "language" | "tool";
  installs: DevInstall[];
  /** Sum of the known install sizes; null when none is known. */
  size: number | null;
}

export interface DevTools {
  languages: DevItem[];
  tools: DevItem[];
}

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
  /** `get_volume`: the scanned root's volume, read fresh on each call. null before a scan. */
  getVolume(): Promise<VolumeInfo | null>;
  /**
   * `get_treemap`: the folder at `path` with children loaded `depth` levels deep (clamped to 1..3).
   * Pass the scan root for the top level. Rejects before a scan or when `path` is not a scanned folder.
   */
  getTreemap(path: string, depth: number): Promise<TreemapNode>;
  /** `preview_cleanup`: checks for the given scanned paths. Rejects before a scan or for a path not in the scan. */
  previewCleanup(paths: string[]): Promise<CleanupPreview>;
  /**
   * `start_cleanup`: re-runs the checks, then moves every non-refused item to the Recycle Bin in the background.
   * Resolves with the batchId at once; progress/result come as `cleanup-progress` / `cleanup-done`.
   * Afterwards the scan data no longer contains the moved items (summary, projects, files and treemap update).
   * Rejects while a scan or another cleanup is running.
   */
  startCleanup(paths: string[]): Promise<string>;
  /** `list_history`: every cleanup entry, newest first. */
  listHistory(): Promise<HistoryEntry[]>;
  /** `restore_entries`: puts items back from the Recycle Bin to their original paths. Not reflected in the scan until a rescan. */
  restoreEntries(ids: number[]): Promise<RestoreResult[]>;
  /** `start_reinstall`: runs the project's reinstall command in its folder; output comes as `reinstall-output` / `reinstall-done`. */
  startReinstall(projectPath: string): Promise<void>;
  onCleanupProgress(cb: (p: CleanupProgress) => void): Promise<() => void>;
  onCleanupDone(cb: (d: CleanupDone) => void): Promise<() => void>;
  onReinstallOutput(cb: (o: ReinstallOutput) => void): Promise<() => void>;
  onReinstallDone(cb: (d: ReinstallDone) => void): Promise<() => void>;
  /** `get_recycle_bin`: fast total for the top-bar indicator. */
  getRecycleBin(): Promise<RecycleBinSummary>;
  /** `list_recycle_bin`: everything in the bin, biggest first. Can take a few seconds for large bins. */
  listRecycleBin(): Promise<RecycleBinItem[]>;
  /**
   * `empty_recycle_bin`: PERMANENTLY deletes everything in the Recycle Bin on all drives, then returns the new summary.
   * Items DevDisk moved there can no longer be put back afterwards.
   */
  emptyRecycleBin(): Promise<RecycleBinSummary>;
  /**
   * `list_apps`: installed desktop and Store apps, biggest first (unknown size last). Works before a scan;
   * an app's folder is measured only when the current scan covers it.
   */
  listApps(): Promise<InstalledApp[]>;
  /**
   * `list_dev_tools`: installed languages (every version) and developer tools, each biggest first.
   * Folders the scan does not cover are measured on the spot, so this can take several seconds.
   */
  listDevTools(): Promise<DevTools>;
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
  cleanupProgress: "cleanup-progress",
  cleanupDone: "cleanup-done",
  reinstallOutput: "reinstall-output",
  reinstallDone: "reinstall-done",
} as const;

/** Tauri command names. Arguments are passed as camelCase objects, e.g. invoke("list_files", { query, sort, descending, limit }). */
export const COMMANDS = {
  startScan: "start_scan",
  cancelScan: "cancel_scan",
  getSummary: "get_summary",
  listProjects: "list_projects",
  listFiles: "list_files",
  getVolume: "get_volume",
  getTreemap: "get_treemap",
  previewCleanup: "preview_cleanup",
  startCleanup: "start_cleanup",
  listHistory: "list_history",
  restoreEntries: "restore_entries",
  startReinstall: "start_reinstall",
  getRecycleBin: "get_recycle_bin",
  listRecycleBin: "list_recycle_bin",
  emptyRecycleBin: "empty_recycle_bin",
  listApps: "list_apps",
  listDevTools: "list_dev_tools",
} as const;
