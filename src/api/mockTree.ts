// Fake folder tree behind the mock getTreemap, kept apart from mock.ts so that file stays about scan/search.
import type { Project, TreemapNode, VolumeInfo } from "./types";

const MB = 1024 * 1024;
const GB = 1024 * MB;
const MAX_CHILDREN = 60;

// Same small seeded PRNG as mock.ts; a separate copy avoids a circular import.
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

interface MNode {
  name: string;
  path: string;
  dir: boolean;
  bytes: number;
  files: number;
  /** Files that exist but are not modelled as nodes, so a folder can claim many files cheaply. */
  extraFiles: number;
  label: string | null;
  projectRoot: boolean;
  regenerable: boolean;
  kids: MNode[];
}

export interface MockTree {
  index: Map<string, MNode>;
}

export function normPath(p: string): string {
  return p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
}

function mkNode(parent: MNode | null, name: string, dir: boolean, rootPath?: string): MNode {
  const n: MNode = {
    name,
    path: rootPath ?? `${parent!.path.replace(/[\\/]+$/, "")}\\${name}`,
    dir,
    bytes: 0,
    files: dir ? 0 : 1,
    extraFiles: 0,
    // Folders inherit the enclosing project's label, like the real contract.
    label: parent?.label ?? null,
    projectRoot: false,
    regenerable: false,
    kids: [],
  };
  parent?.kids.push(n);
  return n;
}

function mkFile(parent: MNode, name: string, bytes: number): MNode {
  const f = mkNode(parent, name, false);
  f.bytes = Math.max(1, Math.round(bytes));
  return f;
}

const REGEN_NAMES: Record<string, string[]> = {
  node_modules: ["typescript", "@types", "react-dom", "webpack", "esbuild", ".pnpm", ".cache", "lodash", "core-js", "@babel", "eslint", "next", "vite", "rollup", "prettier"],
  target: ["debug", "release", ".fingerprint", "deps", "incremental"],
  ".venv": ["Lib", "Scripts", "site-packages", "share"],
  vendor: ["symfony", "laravel", "doctrine", "monolog", "composer"],
};

// Splits `total` bytes across sub-folders with uneven weights, like real dependency trees.
function fillDir(dir: MNode, total: number, names: string[], r: () => number): void {
  const weights = names.map(() => 0.15 + r() ** 2 * 3);
  const sum = weights.reduce((a, b) => a + b, 0);
  names.forEach((nm, i) => {
    const b = (total * weights[i]) / sum;
    const d = mkNode(dir, nm, true);
    mkFile(d, "bundle.bin", b * 0.8);
    mkFile(d, "index.js", b * 0.2);
    d.extraFiles = Math.round(b / (14 * 1024));
  });
}

function finalize(n: MNode): void {
  if (!n.dir) return;
  n.kids.forEach(finalize);
  n.kids.sort((a, b) => b.bytes - a.bytes);
  n.bytes = n.kids.reduce((a, k) => a + k.bytes, 0);
  n.files = n.extraFiles + n.kids.reduce((a, k) => a + k.files, 0);
}

/** `sourceMb` maps project name to its source size so the tree agrees with the project list. */
export function buildTree(root: string, projs: Project[], sourceMb: Map<string, number>): MockTree {
  const r = rng(2024);
  const base = root.replace(/[\\/]+$/, "");
  const top = mkNode(null, "", true, root);
  const subdir = (parent: MNode, name: string): MNode =>
    parent.kids.find((k) => k.dir && k.name.toLowerCase() === name.toLowerCase()) ?? mkNode(parent, name, true);

  // Shortest paths first so a monorepo exists before the apps nested inside it.
  const ordered = [...projs].sort((a, b) => a.path.length - b.path.length);
  for (const p of ordered) {
    const segs = p.path.slice(base.length + 1).split("\\");
    let cur = top;
    segs.forEach((seg, i) => {
      cur = subdir(cur, seg);
      if (i === segs.length - 1) {
        cur.label = p.label;
        cur.projectRoot = true;
      }
    });
    const src = mkNode(cur, "src", true);
    const sourceBytes = (sourceMb.get(p.name) ?? 2) * MB;
    ["main", "index", "app", "utils", "routes", "types"].forEach((stem, i) => {
      mkFile(src, `${stem}.ts`, (sourceBytes / 6) * (0.4 + r() * 1.2) * (i === 0 ? 1.5 : 1));
    });
    src.extraFiles = Math.round(sourceBytes / (6 * 1024));
    mkFile(cur, "README.md", 4096 * (1 + Math.floor(r() * 3)));
    if (p.hasLockfile) mkFile(cur, "lockfile", 120 * 1024 * (1 + r() * 8));
    for (const reg of p.regenerable) {
      const d = mkNode(cur, reg.name, true);
      d.regenerable = true;
      const names = REGEN_NAMES[reg.name] ?? ["cache", "bin", "lib", "tmp"];
      fillDir(d, reg.allocated, names.slice(0, 3 + Math.floor(r() * names.length)), r);
    }
  }

  // Non-project clutter: plain folders, big files, and one folder of hundreds of tiny files.
  const downloads = mkNode(top, "Downloads", true);
  const dl: [string, number][] = [["windows-11.iso", 5.2 * GB], ["dataset.zip", 1.4 * GB], ["setup-installer.exe", 380 * MB], ["backup-2023.zip", 2.4 * GB], ["slides.pdf", 48 * MB]];
  dl.forEach(([n, b]) => mkFile(downloads, n, b));
  const videos = mkNode(top, "Videos", true);
  const vd: [string, number][] = [["footage-2024.mkv", 4.2 * GB], ["render-final.mp4", 1.1 * GB], ["screen-capture.mp4", 640 * MB]];
  vd.forEach(([n, b]) => mkFile(videos, n, b));
  const cache = mkNode(top, "photos-cache", true);
  for (let i = 0; i < 420; i++) {
    mkFile(cache, `thumb-${String(i + 1).padStart(4, "0")}.jpg`, 4096 + Math.floor(r() * 70 * 1024));
  }
  const docs = mkNode(top, "Documents", true);
  ["taxes", "contracts", "notes"].forEach((nm) => {
    const d = mkNode(docs, nm, true);
    for (let i = 0; i < 5; i++) mkFile(d, `doc-${i + 1}.pdf`, (2 + r() * 30) * MB);
  });
  mkNode(top, "empty-folder", true);
  mkFile(top, "disk-image.vhdx", 3.1 * GB);
  mkFile(top, "todo.txt", 2048);

  finalize(top);
  const index = new Map<string, MNode>();
  const walk = (n: MNode) => {
    if (!n.dir) return;
    index.set(normPath(n.path), n);
    n.kids.forEach(walk);
  };
  walk(top);
  return { index };
}

function toTreemap(n: MNode, depth: number): TreemapNode {
  let children: TreemapNode[] | null = null;
  if (n.dir && depth >= 1) {
    const real = n.kids.filter((k) => k.bytes > 0);
    const shown = real.slice(0, MAX_CHILDREN).map((k) => toTreemap(k, depth - 1));
    const rest = [...real.slice(MAX_CHILDREN), ...n.kids.filter((k) => k.bytes === 0)];
    if (rest.length > 0) {
      shown.push({
        path: n.path,
        name: `${rest.length.toLocaleString("en-US")} smaller items`,
        kind: "others",
        allocated: rest.reduce((a, k) => a + k.bytes, 0),
        fileCount: rest.reduce((a, k) => a + k.files, 0),
        label: n.label,
        projectRoot: false,
        regenerable: false,
        children: null,
      });
    }
    children = shown;
  }
  return {
    path: n.path,
    name: n.name || n.path,
    kind: n.dir ? "dir" : "file",
    allocated: n.bytes,
    fileCount: n.files,
    label: n.label,
    projectRoot: n.projectRoot,
    regenerable: n.regenerable,
    children,
  };
}

/** Throws a string, like the real command rejects with one. */
export function mockTreemap(tree: MockTree | null, path: string, depth: number): TreemapNode {
  if (!tree) throw "No scan has finished yet.";
  const node = tree.index.get(normPath(path));
  if (!node) throw `Not a scanned folder: ${path}`;
  return toTreemap(node, Math.min(3, Math.max(1, Math.round(depth))));
}

export function mockVolume(root: string): VolumeInfo {
  const m = /^([A-Za-z]):/.exec(root.trim());
  return { mount: `${m ? m[1].toUpperCase() : "C"}:\\`, total: 1024 * GB, free: 180 * GB };
}

/** Removes a file or folder from the fake tree and shrinks every ancestor, like a rescan after a cleanup. No-op for unknown paths. */
export function removeFromTree(tree: MockTree | null, path: string): void {
  if (!tree) return;
  const parentOf = (p: string) => p.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "");
  const key = normPath(path);
  const parent = tree.index.get(normPath(parentOf(path)));
  const node = parent?.kids.find((k) => normPath(k.path) === key);
  if (!parent || !node) return;
  parent.kids = parent.kids.filter((k) => k !== node);
  const forget = (n: MNode) => {
    tree.index.delete(normPath(n.path));
    n.kids.forEach(forget);
  };
  forget(node);
  for (let cur: MNode | undefined = parent; cur; cur = tree.index.get(normPath(parentOf(cur.path)))) {
    cur.bytes -= node.bytes;
    cur.files -= node.files;
  }
}
