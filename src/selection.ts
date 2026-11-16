import type { Project } from "./api/types";

export type SelectionSource = "projects" | "files" | "map";

export interface SelectedItem {
  path: string;
  name: string;
  kind: "file" | "dir";
  /** Size on disk, bytes. */
  allocated: number;
  projectPath: string | null;
  projectLabel: string | null;
  source: SelectionSource;
}

/** Keyed by pathKey(path), so the same location picked from two tabs is one entry. */
export type Selection = Map<string, SelectedItem>;

/** Windows paths compare case-insensitively; only ASCII is folded so the rule stays predictable. */
export function pathKey(p: string): string {
  return p
    .replace(/\//g, "\\")
    .replace(/\\+$/, "")
    .replace(/[A-Z]/g, (c) => c.toLowerCase());
}

export function coveringItem(sel: Selection, path: string): SelectedItem | null {
  const k = pathKey(path);
  for (const [key, item] of sel) {
    if (item.kind === "dir" && k.startsWith(`${key}\\`)) return item;
  }
  return null;
}

export type SelectionState = "selected" | "covered" | null;

export function selectionState(sel: Selection, path: string): SelectionState {
  if (sel.has(pathKey(path))) return "selected";
  return coveringItem(sel, path) ? "covered" : null;
}

/** Adds items. A folder replaces anything selected inside it; items inside a selected folder are ignored. */
export function withItems(sel: Selection, items: SelectedItem[]): Selection {
  const next = new Map(sel);
  for (const item of items) {
    const key = pathKey(item.path);
    if (next.has(key) || coveringItem(next, item.path)) continue;
    if (item.kind === "dir") {
      for (const k of [...next.keys()]) if (k.startsWith(`${key}\\`)) next.delete(k);
    }
    next.set(key, item);
  }
  return next;
}

export function withoutPaths(sel: Selection, paths: string[]): Selection {
  const next = new Map(sel);
  for (const p of paths) next.delete(pathKey(p));
  return next;
}

export function totalBytes(sel: Selection): number {
  let n = 0;
  for (const it of sel.values()) n += it.allocated;
  return n;
}

export function projectItems(p: Project): SelectedItem[] {
  return p.regenerable.map((f) => ({
    path: f.path,
    name: f.name,
    kind: "dir",
    allocated: f.allocated,
    projectPath: p.path,
    projectLabel: p.label,
    source: "projects",
  }));
}
