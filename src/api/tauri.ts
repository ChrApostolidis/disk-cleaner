import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  COMMANDS,
  EVENTS,
  type DevDiskApi,
  type Project,
  type FileEntry,
  type ScanError,
  type ScanProgress,
  type ScanSummary,
  type TreemapNode,
  type VolumeInfo,
} from "./types";

export const tauriApi: DevDiskApi = {
  startScan: (root) => invoke<void>(COMMANDS.startScan, { root }),
  cancelScan: () => invoke<void>(COMMANDS.cancelScan),
  getSummary: () => invoke<ScanSummary | null>(COMMANDS.getSummary),
  listProjects: (query) => invoke<Project[]>(COMMANDS.listProjects, { query }),
  listFiles: (query, sort, descending, limit) =>
    invoke<FileEntry[]>(COMMANDS.listFiles, { query, sort, descending, limit }),
  getVolume: () => invoke<VolumeInfo | null>(COMMANDS.getVolume),
  getTreemap: (path, depth) => invoke<TreemapNode>(COMMANDS.getTreemap, { path, depth }),
  onProgress: (cb) => listen<ScanProgress>(EVENTS.progress, (e) => cb(e.payload)),
  onDone: (cb) => listen<ScanSummary>(EVENTS.done, (e) => cb(e.payload)),
  onError: (cb) => listen<ScanError>(EVENTS.error, (e) => cb(e.payload)),
};
