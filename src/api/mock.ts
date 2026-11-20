// Browser-only stand-in for the Rust backend so the UI can be developed with `npm run dev`.
import type {
  CleanupDone,
  CleanupPreview,
  CleanupPreviewItem,
  CleanupProgress,
  CleanupResultItem,
  DevDiskApi,
  DevInstall,
  DevItem,
  DupProgress,
  DuplicateGroup,
  Duplicates,
  FileEntry,
  FileSort,
  HistoryEntry,
  InstalledApp,
  Project,
  RecycleBinItem,
  RecycleBinSummary,
  ReinstallDone,
  ReinstallOffer,
  ReinstallOutput,
  RestoreResult,
  ScanError,
  ScanProgress,
  ScanSummary,
  VolumeInfo,
} from "./types";
import { buildTree, mockTreemap, mockVolume, normPath, removeFromTree, type MockTree } from "./mockTree";

export const MOCK_DEFAULT_ROOT = "C:\\Users\\you\\Projects";
const MB = 1024 * 1024;
const DAY = 86_400_000;

// Small seeded PRNG so the fake data is identical on every reload.
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

interface Seed {
  name: string;
  label: string;
  framework: string | null;
  version: string | null;
  eco: string[];
  pm: string | null;
  lock: boolean;
  folders: [string, number][]; // name, MB
  sourceMb: number;
  idleDays: number;
  parent?: number; // index into SEEDS
}

const SEEDS: Seed[] = [
  { name: "my-shop", label: "Next.js", framework: "Next.js", version: "14.2.3", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 812], [".next", 240]], sourceMb: 6, idleDays: 200 },
  { name: "portfolio-app", label: "React (Vite)", framework: "React", version: "18.3.1", eco: ["node"], pm: "pnpm", lock: true, folders: [["node_modules", 410], ["dist", 12]], sourceMb: 3, idleDays: 12 },
  { name: "devdisk", label: "Tauri", framework: "Tauri", version: "2.1.0", eco: ["node", "rust"], pm: "npm", lock: true, folders: [["node_modules", 280]], sourceMb: 4, idleDays: 0 },
  { name: "src-tauri", label: "Rust", framework: null, version: null, eco: ["rust"], pm: "cargo", lock: true, folders: [["target", 5400]], sourceMb: 1, idleDays: 0, parent: 2 },
  { name: "invoice-api", label: "Laravel", framework: "Laravel", version: "11.9.0", eco: ["php", "node"], pm: "composer", lock: true, folders: [["vendor", 96], ["node_modules", 330]], sourceMb: 8, idleDays: 400 },
  { name: "ml-notebooks", label: "Python", framework: null, version: null, eco: ["python"], pm: "pip", lock: false, folders: [[".venv", 2100], ["__pycache__", 4]], sourceMb: 40, idleDays: 150 },
  { name: "blog-backend", label: "Django", framework: "Django", version: "5.0.6", eco: ["python"], pm: "pip", lock: false, folders: [[".venv", 380]], sourceMb: 5, idleDays: 90 },
  { name: "cli-tools", label: "Rust", framework: null, version: null, eco: ["rust"], pm: "cargo", lock: true, folders: [["target", 1900]], sourceMb: 2, idleDays: 30 },
  { name: "chat-server", label: "Node.js (Express)", framework: "Express", version: "4.19.2", eco: ["node"], pm: "yarn", lock: true, folders: [["node_modules", 190]], sourceMb: 2, idleDays: 520 },
  { name: "landing-page", label: "Astro", framework: "Astro", version: "4.9.1", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 260], ["dist", 8]], sourceMb: 5, idleDays: 60 },
  { name: "mobile-app", label: "React Native", framework: "React Native", version: "0.74.1", eco: ["node"], pm: "yarn", lock: true, folders: [["node_modules", 940]], sourceMb: 12, idleDays: 75 },
  { name: "scripts", label: "Node.js", framework: null, version: null, eco: ["node"], pm: null, lock: false, folders: [["node_modules", 45]], sourceMb: 1, idleDays: 700 },
  { name: "admin-panel", label: "Angular", framework: "Angular", version: "17.3.8", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 620], [".angular", 90]], sourceMb: 9, idleDays: 120 },
  { name: "docs-site", label: "Nuxt", framework: "Nuxt", version: "3.11.2", eco: ["node"], pm: "pnpm", lock: true, folders: [["node_modules", 350], [".nuxt", 60]], sourceMb: 4, idleDays: 20 },
  { name: "api-gateway", label: "NestJS", framework: "NestJS", version: "10.3.8", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 280], ["dist", 5]], sourceMb: 3, idleDays: 45 },
  { name: "billing-service", label: "Spring Boot", framework: "Spring Boot", version: "3.2.5", eco: ["java"], pm: "maven", lock: false, folders: [["target", 140]], sourceMb: 3, idleDays: 260 },
  { name: "ml-gateway", label: "FastAPI", framework: "FastAPI", version: null, eco: ["python"], pm: "pip", lock: false, folders: [[".venv", 520]], sourceMb: 1, idleDays: 15 },
  { name: "dashboard", label: "React", framework: "React", version: "18.2.0", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 390], ["build", 6]], sourceMb: 2, idleDays: 330 },
  { name: "notes-desktop", label: "Electron", framework: "Electron", version: "30.0.6", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 720], ["out", 180]], sourceMb: 3, idleDays: 240 },
  { name: "game-of-life", label: "Rust", framework: null, version: null, eco: ["rust"], pm: "cargo", lock: true, folders: [["target", 640]], sourceMb: 1, idleDays: 410 },
  { name: "monorepo", label: "Node.js", framework: null, version: null, eco: ["node"], pm: "pnpm", lock: true, folders: [["node_modules", 1200]], sourceMb: 15, idleDays: 8 },
  { name: "web", label: "Remix", framework: "Remix", version: "2.9.2", eco: ["node"], pm: "pnpm", lock: true, folders: [["build", 10]], sourceMb: 3, idleDays: 8, parent: 20 },
  { name: "svelte-kit-demo", label: "SvelteKit", framework: "SvelteKit", version: "2.5.10", eco: ["node"], pm: "npm", lock: true, folders: [["node_modules", 210], [".svelte-kit", 20]], sourceMb: 2, idleDays: 95 },
  { name: "vue-playground", label: "Vue", framework: "Vue", version: "3.4.27", eco: ["node"], pm: "npm", lock: false, folders: [["node_modules", 170]], sourceMb: 1, idleDays: 280 },
  { name: "legacy-cms", label: "Symfony", framework: "Symfony", version: "6.4.7", eco: ["php"], pm: "composer", lock: true, folders: [["vendor", 130], ["var", 220]], sourceMb: 20, idleDays: 900 },
  { name: "flask-api", label: "Flask", framework: "Flask", version: null, eco: ["python"], pm: "pip", lock: false, folders: [[".venv", 150]], sourceMb: 1, idleDays: 600 },
  { name: "build-tools", label: "Gradle", framework: "Gradle", version: null, eco: ["java"], pm: "gradle", lock: false, folders: [[".gradle", 310], ["build", 80]], sourceMb: 2, idleDays: 180 },
];

const GROUPS = ["work", "personal", "clients\\acme", "experiments", "archive"];

function buildProjects(root: string, now: number): Project[] {
  const r = rng(7);
  const built: Project[] = SEEDS.map((s, i) => ({
    id: i,
    path: "",
    name: s.name,
    displayName: `${s.framework ?? s.label} project: ${s.name}`,
    label: s.label,
    framework: s.framework,
    version: s.version,
    ecosystems: s.eco,
    packageManager: s.pm,
    hasLockfile: s.lock,
    reinstallCommand: s.pm ? `${s.pm} install` : null,
    regenerable: [],
    totalAllocated: 0,
    reclaimable: 0,
    lastActivity: s.idleDays === 0 ? now - 3 * 3_600_000 : now - s.idleDays * DAY,
    parentId: s.parent ?? null,
  }));
  // Parents come before their children in SEEDS, so their paths exist when needed.
  built.forEach((p, i) => {
    const s = SEEDS[i];
    const parent = p.parentId === null ? null : built[p.parentId];
    p.path = parent
      ? `${parent.path}\\${s.name === "src-tauri" ? "src-tauri" : `apps\\${s.name}`}`
      : `${root}\\${GROUPS[Math.floor(r() * GROUPS.length)]}\\${s.name}`;
    p.regenerable = s.folders.map(([name, mb]) => ({
      name,
      path: `${p.path}\\${name}`,
      allocated: Math.round(mb * MB * (0.95 + r() * 0.1)),
    }));
    p.reclaimable = p.regenerable.reduce((a, f) => a + f.allocated, 0);
    p.totalAllocated = p.reclaimable + Math.round(s.sourceMb * MB);
  });
  const sorted = [...built].sort((a, b) => b.reclaimable - a.reclaimable);
  // ids are indexes into the returned list, so re-map them after sorting.
  const newId = new Map(sorted.map((p, j) => [p.id, j]));
  return sorted.map((p, j) => ({
    ...p,
    id: j,
    parentId: p.parentId === null ? null : (newId.get(p.parentId) ?? null),
  }));
}

const EXTS: [string, number][] = [
  ["mp4", 900], ["mkv", 2200], ["iso", 3500], ["zip", 300], ["psd", 400], ["blend", 250],
  ["log", 40], ["json", 8], ["png", 6], ["pdf", 20], ["dll", 15], ["bin", 120], ["db", 200], ["txt", 1],
];
const STEMS = ["backup", "export", "render", "footage", "archive", "dump", "report", "scan", "capture", "dataset", "image", "build", "release", "notes", "final", "draft"];
const SUBDIRS = ["media", "downloads", "projects", "tmp", "data"];

function buildFiles(root: string, now: number): FileEntry[] {
  const r = rng(99);
  const out: FileEntry[] = [];
  for (let i = 0; i < 5000; i++) {
    const [ext, scale] = EXTS[Math.floor(r() * EXTS.length)];
    // Heavy-tailed so a handful of files dominate, like a real disk.
    const size = Math.max(512, Math.round(scale * MB * Math.pow(r(), 4) * 2));
    const name = `${STEMS[Math.floor(r() * STEMS.length)]}-${Math.floor(r() * 9000 + 1000)}.${ext}`;
    const dir = `${root}\\${GROUPS[Math.floor(r() * GROUPS.length)]}\\${SUBDIRS[Math.floor(r() * SUBDIRS.length)]}`;
    out.push({
      path: `${dir}\\${name}`,
      name,
      size,
      allocated: Math.ceil(size / 4096) * 4096,
      modified: r() < 0.02 ? null : now - Math.floor(r() * 1100) * DAY,
    });
  }
  return out;
}

/** Copies of some bigger files in backup and download folders, so the duplicate finder has something to find. */
function withCopies(list: FileEntry[], root: string): FileEntry[] {
  const r = rng(7);
  const places = [`${root}\\archive\\old-backup`, `${root}\\personal\\downloads`, `${root}\\work\\media\\copy`];
  const copies: FileEntry[] = [];
  // Every path must stay unique, as it is on a real disk.
  const taken = new Set(list.map((f) => normPath(f.path)));
  for (const f of list) {
    if (f.size < MB || r() > 0.12) continue;
    const n = r() < 0.25 ? 2 : 1;
    const first = Math.floor(r() * places.length);
    for (let i = 0; i < n; i++) {
      const path = `${places[(first + i) % places.length]}\\${f.name}`;
      if (taken.has(normPath(path))) continue;
      taken.add(normPath(path));
      copies.push({ ...f, path, modified: f.modified === null ? null : f.modified + Math.floor(r() * 90) * DAY });
    }
  }
  return [...list, ...copies];
}

// ---- search syntax ----

interface Filter {
  terms: ((name: string) => boolean)[];
  minSize: number | null;
  maxSize: number | null;
  olderMs: number | null;
  newerMs: number | null;
}

const SIZE_UNITS: Record<string, number> = { B: 1, KB: 1024, MB: MB, GB: 1024 ** 3, TB: 1024 ** 4 };
const TIME_UNITS: Record<string, number> = { m: 60_000, h: 3_600_000, d: DAY, w: 7 * DAY, y: 365 * DAY };

function tokenize(q: string): string[] {
  return (q.match(/"[^"]*"|\S+/g) ?? []).map((t) => t.replace(/^"|"$/g, ""));
}

// Throws a string, like the real commands reject with one.
function parse(query: string): Filter {
  const f: Filter = { terms: [], minSize: null, maxSize: null, olderMs: null, newerMs: null };
  for (const tok of tokenize(query)) {
    const size = /^([<>])(.*)$/.exec(tok);
    if (size) {
      const m = /^(\d+(?:\.\d+)?)\s*(B|KB|MB|GB|TB)$/i.exec(size[2]);
      if (!m) throw `invalid size filter "${tok}": expected a number and a unit, e.g. >500MB`;
      const bytes = parseFloat(m[1]) * SIZE_UNITS[m[2].toUpperCase()];
      if (size[1] === ">") f.minSize = bytes;
      else f.maxSize = bytes;
      continue;
    }
    const age = /^(older|newer):(.*)$/i.exec(tok);
    if (age) {
      const m = /^(\d+)([mhdwy])$/.exec(age[2]);
      if (!m) throw `invalid age filter "${tok}": expected a number and a unit (m, h, d, w, y), e.g. older:90d`;
      const ms = parseInt(m[1], 10) * TIME_UNITS[m[2]];
      if (age[1].toLowerCase() === "older") f.olderMs = ms;
      else f.newerMs = ms;
      continue;
    }
    const lower = tok.toLowerCase();
    if (/[*?]/.test(lower)) {
      const re = new RegExp(
        "^" + lower.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".") + "$",
      );
      f.terms.push((n) => re.test(n.toLowerCase()));
    } else {
      f.terms.push((n) => n.toLowerCase().includes(lower));
    }
  }
  return f;
}

function matches(f: Filter, name: string, allocated: number, modified: number | null, now: number): boolean {
  if (!f.terms.every((t) => t(name))) return false;
  if (f.minSize !== null && !(allocated > f.minSize)) return false;
  if (f.maxSize !== null && !(allocated < f.maxSize)) return false;
  if (f.olderMs !== null && !(modified !== null && now - modified > f.olderMs)) return false;
  if (f.newerMs !== null && !(modified !== null && now - modified < f.newerMs)) return false;
  return true;
}

// ---- backend state ----

type Listeners<T> = Set<(v: T) => void>;
const progressL: Listeners<ScanProgress> = new Set();
const doneL: Listeners<ScanSummary> = new Set();
const errorL: Listeners<ScanError> = new Set();

const cleanupProgressL: Listeners<CleanupProgress> = new Set();
const cleanupDoneL: Listeners<CleanupDone> = new Set();
const reinstallOutL: Listeners<ReinstallOutput> = new Set();
const reinstallDoneL: Listeners<ReinstallDone> = new Set();

let projects: Project[] = [];
let files: FileEntry[] = [];
let summary: ScanSummary | null = null;
let tree: MockTree | null = null;
let volume: VolumeInfo | null = null;
let timer: ReturnType<typeof setInterval> | null = null;

function subscribe<T>(set: Listeners<T>, cb: (v: T) => void): Promise<() => void> {
  boot();
  set.add(cb);
  return Promise.resolve(() => {
    set.delete(cb);
  });
}
const delay = (ms: number) => new Promise<void>((res) => setTimeout(res, ms));

// ---- cleanup, history, reinstall ----

const BIN_LIMIT = 50 * 1024 ** 3;
const CLEANUP_MS = 1500;
let cleaning = false;
let nextEntryId = 1;
let nextBatch = 1;
const reinstalling = new Set<string>();

function seedHistory(): HistoryEntry[] {
  const now = Date.now();
  const base = MOCK_DEFAULT_ROOT;
  const mk = (batchId: string, ago: number, path: string, mb: number, label: string): HistoryEntry => ({
    id: nextEntryId++,
    batchId,
    deletedAt: now - ago,
    path,
    allocated: mb * MB,
    projectPath: path.slice(0, path.lastIndexOf("\\")),
    projectLabel: label,
    restoredAt: null,
  });
  // Older than a day on purpose: putting these back fails with "no longer in the Recycle Bin".
  return [
    mk("seed-a", 3 * DAY, `${base}\\archive\\old-site\\node_modules`, 1130, "Next.js"),
    mk("seed-a", 3 * DAY, `${base}\\archive\\old-site\\.next`, 210, "Next.js"),
    mk("seed-b", 9 * DAY, `${base}\\experiments\\prototype\\target`, 3480, "Rust"),
  ];
}
const history: HistoryEntry[] = seedHistory();

// Throws a string for paths outside the scan, like the real command rejects.
function classify(path: string): CleanupPreviewItem {
  const key = normPath(path);
  for (const p of projects) {
    const f = p.regenerable.find((x) => normPath(x.path) === key);
    if (!f) continue;
    const reasons: string[] = [];
    let verdict: CleanupPreviewItem["verdict"] = "ok";
    if (f.name === "dist" && p.name === "portfolio-app") {
      verdict = "refuse";
      reasons.push("tracked by git: deleting would lose committed files");
    } else if (f.allocated > BIN_LIMIT) {
      verdict = "refuse";
      reasons.push("too big for the Recycle Bin (over 50 GB)");
    } else if (f.name === "node_modules" && !p.hasLockfile) {
      verdict = "warn";
      reasons.push("no lockfile: reinstalling may install different versions");
    } else if (f.name === ".venv" && !p.hasLockfile) {
      verdict = "warn";
      reasons.push("no lockfile: the environment may not be reproducible");
    }
    return {
      path: f.path, name: f.name, kind: "dir", allocated: f.allocated, regenerable: true,
      projectPath: p.path, projectLabel: p.label, verdict, reasons,
    };
  }
  const file = files.find((x) => normPath(x.path) === key);
  if (!file) throw `Not part of the scan: ${path}`;
  const dll = file.name.toLowerCase().endsWith(".dll");
  return {
    path: file.path, name: file.name, kind: "file", allocated: file.allocated, regenerable: false,
    projectPath: null, projectLabel: null,
    verdict: dll ? "warn" : file.allocated > BIN_LIMIT ? "refuse" : "ok",
    reasons: dll ? ["a program may still need this library"] : [],
  };
}

function offerFor(item: CleanupPreviewItem): ReinstallOffer | null {
  if (item.name !== "node_modules" || !item.projectPath) return null;
  const p = projects.find((x) => x.path === item.projectPath);
  if (!p || !p.hasLockfile || !p.packageManager) return null;
  return { projectPath: p.path, command: p.packageManager === "npm" ? "npm ci" : `${p.packageManager} install --frozen-lockfile` };
}

function applyRemoval(removed: CleanupPreviewItem[]): void {
  const gone = new Set(removed.map((i) => normPath(i.path)));
  const freed = removed.reduce((a, i) => a + i.allocated, 0);
  projects = projects.map((p) => {
    const lost = p.regenerable.filter((f) => gone.has(normPath(f.path)));
    if (lost.length === 0) return p;
    const bytes = lost.reduce((a, f) => a + f.allocated, 0);
    return {
      ...p,
      regenerable: p.regenerable.filter((f) => !gone.has(normPath(f.path))),
      reclaimable: p.reclaimable - bytes,
      totalAllocated: p.totalAllocated - bytes,
    };
  });
  files = files.filter((f) => !gone.has(normPath(f.path)));
  removed.forEach((i) => removeFromTree(tree, i.path));
  if (summary) {
    summary = {
      ...summary,
      allocated: summary.allocated - freed,
      size: Math.max(0, summary.size - Math.round(freed * 0.97)),
      reclaimable: projects.reduce((a, p) => a + p.reclaimable, 0),
    };
  }
  if (volume) volume = { ...volume, free: volume.free + freed };
}

// ---- duplicates ----

const DUP_MS = 2500;
const dupProgressL: Listeners<DupProgress> = new Set();
const dupDoneL: Listeners<null> = new Set();
const dupErrorL: Listeners<ScanError> = new Set();
let dupTimer: ReturnType<typeof setInterval> | null = null;
/** Scan the last search ran on, and when it finished. */
let dupFor: { scannedAt: number; foundAt: number } | null = null;

/** Copies share name and size in the mock data, which stands in for "same content". */
function dupGroups(): FileEntry[][] {
  const by = new Map<string, FileEntry[]>();
  for (const f of files) {
    if (f.size < MB) continue;
    const k = `${f.name.toLowerCase()}|${f.size}`;
    by.set(k, [...(by.get(k) ?? []), f]);
  }
  return [...by.values()].filter((g) => g.length > 1);
}

function stopDuplicates(message: string): void {
  if (dupTimer === null) return;
  clearInterval(dupTimer);
  dupTimer = null;
  dupErrorL.forEach((cb) => cb({ message, cancelled: true }));
}

// ---- Recycle Bin ----

const BIN_NAMES = ["node_modules", "target", ".venv", "dist", "build", ".next", "vendor", "cache", "report.pdf", "backup.zip", "render.mp4", "notes.txt", "screenshot.png", "setup.exe", "dump.sql", "old-photos", "Downloads", "draft.docx"];
let bin: RecycleBinItem[] = [];

function seedBin(): RecycleBinItem[] {
  const r = rng(31);
  const now = Date.now();
  const out: RecycleBinItem[] = history.slice(0, 3).map((h, i) => ({
    id: `bin-h${i}`,
    name: h.path.slice(h.path.lastIndexOf("\\") + 1),
    originalPath: h.path,
    deletedAt: h.deletedAt,
    kind: "dir",
    size: h.allocated,
    fromDevDisk: true,
  }));
  for (let i = 0; i < 57; i++) {
    const name = BIN_NAMES[Math.floor(r() * BIN_NAMES.length)];
    const dir = name === "node_modules" || name === "target" || name === ".venv" || name === "dist" || name === "build" || name === ".next" || name === "vendor" || name === "cache";
    const big = name === "node_modules" ? 200 + r() * 900 : dir ? 20 + r() * 400 : 0.05 + Math.pow(r(), 3) * 300;
    const parent = `C:\\Users\\you\\${["old-projects", "tmp", "Desktop", "Documents\\work", "school"][Math.floor(r() * 5)]}`;
    out.push({
      id: `bin-${i}`,
      name,
      originalPath: `${parent}\\${dir ? `proj-${Math.floor(r() * 90 + 10)}\\` : ""}${name}`,
      deletedAt: now - Math.floor(r() * 60 * DAY) - 5 * 60_000,
      kind: dir ? "dir" : "file",
      size: r() < 0.1 ? null : Math.round(big * MB),
      fromDevDisk: false,
    });
  }
  return out;
}
bin = seedBin();

function binSummary(): RecycleBinSummary {
  return { items: bin.length, size: bin.reduce((a, i) => a + (i.size ?? 0), 0) };
}

// Stands in for the scan the real backend restores from disk at startup.
let booted = false;
let bootTimer: ReturnType<typeof setTimeout> | null = null;
function boot(): void {
  if (booted) return;
  booted = true;
  const root = MOCK_DEFAULT_ROOT;
  const at = new Date(Date.now() - DAY);
  at.setHours(18, 40, 0, 0);
  const scannedAt = at.getTime();
  bootTimer = setTimeout(() => {
    bootTimer = null;
    const builtProjects = buildProjects(root, scannedAt);
    const builtFiles = withCopies(buildFiles(root, scannedAt), root);
    projects = builtProjects;
    files = builtFiles;
    tree = buildTree(root, builtProjects, new Map(SEEDS.map((x) => [x.name, x.sourceMb])));
    volume = mockVolume(root);
    const alloc = builtFiles.reduce((a, f) => a + f.allocated, 0) + builtProjects.reduce((a, p) => a + p.totalAllocated, 0);
    const s: ScanSummary = {
      root, files: 184_320, dirs: 21_450, allocated: alloc, size: Math.round(alloc * 0.97), elapsedMs: 38_400,
      issues: 3, projects: builtProjects.length, reclaimable: builtProjects.reduce((a, p) => a + p.reclaimable, 0), scannedAt,
    };
    summary = s;
    doneL.forEach((cb) => cb(s));
  }, 400);
}

const REINSTALL_LINES: [ReinstallOutput["stream"], string][] = [
  ["stdout", "> npm ci"],
  ["stdout", ""],
  ["stderr", "npm warn deprecated inflight@1.0.6: This module is not supported, and leaks memory."],
  ["stderr", "npm warn deprecated glob@7.2.3: Glob versions prior to v9 are no longer supported"],
  ["stdout", "reify:typescript: http fetch GET 200 https://registry.npmjs.org/typescript 84ms"],
  ["stdout", "reify:react: http fetch GET 200 https://registry.npmjs.org/react 61ms"],
  ["stdout", "reify:vite: http fetch GET 200 https://registry.npmjs.org/vite 102ms"],
  ["stdout", "reify:esbuild: timing reifyNode:node_modules/esbuild Completed in 412ms"],
  ["stdout", "reify:rollup: timing reifyNode:node_modules/rollup Completed in 530ms"],
  ["stdout", "reify:@types/node: timing reifyNode:node_modules/@types/node Completed in 98ms"],
  ["stdout", "run esbuild postinstall: node install.js"],
  ["stdout", ""],
  ["stdout", "added 1432 packages, and audited 1433 packages in 2s"],
  ["stdout", "found 0 vulnerabilities"],
  ["stdout", "done"],
];

// [name, publisher, version, kind, folder (null = unknown), MB (null = unknown), measured by the scan, days since install]
const APP_SEEDS: [string, string | null, string | null, InstalledApp["kind"], string | null, number | null, boolean, number | null][] = [
  ["Docker Desktop", "Docker Inc.", "4.48.0", "desktop", "C:\\Program Files\\Docker\\Docker", 2867, true, 40],
  ["Microsoft Visual Studio Code (User)", "Microsoft Corporation", "1.106.0", "desktop", "C:\\Users\\you\\AppData\\Local\\Programs\\Microsoft VS Code", 1082, true, 12],
  ["Android Studio", "Google LLC", "2026.1", "desktop", "C:\\Program Files\\Android\\Android Studio", 3410, false, null],
  ["Node.js", "Node.js Foundation", "24.11.0", "desktop", "C:\\Program Files\\nodejs", 98, true, 90],
  ["Git", "The Git Development Community", "2.45.2", "desktop", "C:\\Program Files\\Git", 340, false, 400],
  ["PostgreSQL 18", "PostgreSQL Global Development Group", "18.0", "desktop", "C:\\Program Files\\PostgreSQL\\18", 1047, false, 70],
  ["7-Zip 24.09 (x64)", "Igor Pavlov", "24.09", "desktop", "C:\\Program Files\\7-Zip", 5.6, false, null],
  ["Obsidian", "Obsidian", "1.9.12", "desktop", "C:\\Users\\you\\AppData\\Local\\Programs\\Obsidian", 1104, true, 200],
  ["Microsoft .NET SDK 10.0.204 (x64)", "Microsoft Corporation", "10.0.204", "desktop", null, 1204, false, 30],
  ["Some Old Tool", null, null, "desktop", null, null, false, null],
  ["Windows Terminal", null, "1.23.12811.0", "store", "C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.23.12811.0_x64__8wekyb3d8bbwe", null, false, null],
  ["Spotify", null, "1.2.74.477.0", "store", "C:\\Program Files\\WindowsApps\\SpotifyAB.SpotifyMusic_1.2.74.477.0_x86__zpdnekdrzrea0", null, false, null],
  ["Calculator", null, "11.2508.4.0", "store", "C:\\Program Files\\WindowsApps\\Microsoft.WindowsCalculator_11.2508.4.0_x64__8wekyb3d8bbwe", null, false, null],
];

function mockApps(now: number): InstalledApp[] {
  const apps = APP_SEEDS.map(([name, publisher, version, kind, location, mb, scanned, days], i): InstalledApp => ({
    id: `mock-app-${i}`,
    name,
    publisher,
    version,
    kind,
    location,
    installedAt: days === null ? null : Math.floor((now - days * DAY) / DAY) * DAY,
    size: mb === null ? null : Math.round(mb * MB),
    sizeSource: mb === null ? null : scanned && summary ? "scan" : "windows",
  }));
  return apps.sort((a, b) => (b.size ?? -1) - (a.size ?? -1) || a.name.localeCompare(b.name));
}

const GB = 1024 * MB;

function devItem(id: string, name: string, kind: DevItem["kind"], installs: Partial<DevInstall>[]): DevItem {
  const full = installs.map(
    (i): DevInstall => ({ label: "", version: null, path: null, active: false, size: null, sizeSource: null, ...i }),
  );
  const known = full.filter((i) => i.size !== null);
  return { id, name, kind, installs: full, size: known.length ? known.reduce((a, i) => a + (i.size ?? 0), 0) : null };
}

function bySize(items: DevItem[]): DevItem[] {
  return items.sort((a, b) => (b.size ?? -1) - (a.size ?? -1) || a.name.localeCompare(b.name));
}

const HOME = "C:\\Users\\you";

function mockDevTools() {
  const languages = bySize([
    devItem("node", "Node.js", "language", [
      { label: "v24.22.1", version: "24.22.1", path: `${HOME}\\AppData\\Local\\nvm\\v24.22.1`, active: true, size: 0.36 * GB, sizeSource: "measured" },
      { label: "v20.11.0", version: "20.11.0", path: `${HOME}\\AppData\\Local\\nvm\\v20.11.0`, size: 0.31 * GB, sizeSource: "measured" },
      { label: "v16.20.2", version: "16.20.2", path: `${HOME}\\AppData\\Local\\nvm\\v16.20.2`, size: 0.27 * GB, sizeSource: "measured" },
    ]),
    devItem(".net", ".NET SDK", "language", [
      { label: "10.0.204", version: "10.0.204", path: "C:\\Program Files\\dotnet\\sdk\\10.0.204", active: true, size: 0.43 * GB, sizeSource: "measured" },
      { label: "9.0.201", version: "9.0.201", path: "C:\\Program Files\\dotnet\\sdk\\9.0.201", size: 0.41 * GB, sizeSource: "measured" },
      { label: "8.0.404", version: "8.0.404", path: "C:\\Program Files\\dotnet\\sdk\\8.0.404", size: 0.39 * GB, sizeSource: "measured" },
    ]),
    devItem("rust", "Rust", "language", [
      { label: "stable-x86_64-pc-windows-msvc", version: "1.100.0", path: `${HOME}\\.rustup\\toolchains\\stable-x86_64-pc-windows-msvc`, active: true, size: 1.7 * GB, sizeSource: "measured" },
    ]),
    devItem("python", "Python", "language", [
      { label: "Python312", version: "3.12.4", path: `${HOME}\\AppData\\Local\\Programs\\Python\\Python312`, active: true, size: 0.22 * GB, sizeSource: "scan" },
      { label: "Python311", version: "3.11.9", path: `${HOME}\\AppData\\Local\\Programs\\Python\\Python311`, size: 0.21 * GB, sizeSource: "scan" },
    ]),
    devItem("java", "Java", "language", [
      { label: "jdk-25", version: "25.0.1", path: "C:\\Program Files\\Java\\jdk-25", active: true, size: 0.38 * GB, sizeSource: "measured" },
    ]),
    devItem("php", "PHP", "language", [
      { label: "php-8.5.4", version: "8.5.4", path: "C:\\php-8.5.4", active: true, size: 0.1 * GB, sizeSource: "measured" },
      { label: "php", version: "8.2.12", path: "C:\\xampp\\php", size: 0.19 * GB, sizeSource: "measured" },
    ]),
  ]);
  const tools = bySize([
    devItem("android", "Android", "tool", [
      { label: "Android Studio", version: "2026.1", path: "C:\\Program Files\\Android\\Android Studio", size: 3.4 * GB, sizeSource: "windows" },
      { label: "Android SDK", path: `${HOME}\\AppData\\Local\\Android\\Sdk`, size: 10.45 * GB, sizeSource: "measured" },
      { label: "Emulators (AVDs)", path: `${HOME}\\.android\\avd`, size: 7.69 * GB, sizeSource: "measured" },
    ]),
    devItem("docker", "Docker Desktop", "tool", [
      { label: "Docker Desktop", version: "4.48.0", path: "C:\\Program Files\\Docker\\Docker", size: 2.8 * GB, sizeSource: "windows" },
      { label: "Docker disks (images, containers, volumes)", path: `${HOME}\\AppData\\Local\\Docker\\wsl`, size: 4.68 * GB, sizeSource: "measured" },
    ]),
    devItem("vscode", "Visual Studio Code", "tool", [
      { label: "Microsoft Visual Studio Code (User)", version: "1.106.0", path: `${HOME}\\AppData\\Local\\Programs\\Microsoft VS Code`, size: 1.08 * GB, sizeSource: "scan" },
      { label: "Extensions", path: `${HOME}\\.vscode\\extensions`, size: 1.04 * GB, sizeSource: "measured" },
    ]),
    devItem("git", "Git", "tool", [
      { label: "Git", version: "2.45.2", path: "C:\\Program Files\\Git", size: 0.34 * GB, sizeSource: "windows" },
    ]),
    devItem("visual-studio", "Visual Studio", "tool", [{ label: "Visual Studio Community 2022", version: "17.14.4" }]),
  ]);
  return { languages, tools };
}

export const mockApi: DevDiskApi = {
  async listDevTools() {
    await delay(1200);
    return mockDevTools();
  },
  async listApps() {
    await delay(500);
    return mockApps(Date.now());
  },
  async getRecycleBin() {
    boot();
    await delay(60);
    return binSummary();
  },
  async listRecycleBin() {
    await delay(700);
    return [...bin].sort((a, b) => (b.size ?? -1) - (a.size ?? -1)).map((i) => ({ ...i }));
  },
  async emptyRecycleBin() {
    await delay(1000);
    bin = [];
    return binSummary();
  },
  async previewCleanup(paths): Promise<CleanupPreview> {
    await delay(150);
    if (!summary) throw "No scan has finished yet.";
    const items = [...new Set(paths)].map(classify);
    if (dupFor && summary && dupFor.scannedAt === summary.scannedAt) {
      const picked = new Set(items.map((i) => normPath(i.path)));
      for (const g of dupGroups()) {
        if (!g.every((f) => picked.has(normPath(f.path)))) continue;
        for (const it of items) {
          if (!g.some((f) => normPath(f.path) === normPath(it.path))) continue;
          it.reasons.push("every copy of this duplicate is selected: no copy would be kept");
          if (it.verdict === "ok") it.verdict = "warn";
        }
      }
    }
    return { items, totalFreed: items.filter((i) => i.verdict !== "refuse").reduce((a, i) => a + i.allocated, 0) };
  },
  async startCleanup(paths) {
    if (timer !== null) throw "A scan is running.";
    if (cleaning) throw "Another cleanup is already running.";
    const items = [...new Set(paths)].map(classify);
    const batchId = `batch-${nextBatch++}`;
    const todo = items.filter((i) => i.verdict !== "refuse");
    const start = Date.now();
    cleaning = true;
    const tick = setInterval(() => {
      const t = Math.min(1, (Date.now() - start) / CLEANUP_MS);
      if (t < 1) {
        const done = Math.floor(t * todo.length);
        const p: CleanupProgress = {
          batchId, done, total: todo.length, currentPath: todo[Math.min(done, todo.length - 1)]?.path ?? "",
        };
        cleanupProgressL.forEach((cb) => cb(p));
        return;
      }
      clearInterval(tick);
      const now = Date.now();
      const results: CleanupResultItem[] = items.map((i) => {
        if (i.verdict === "refuse") return { path: i.path, ok: false, error: `Skipped: ${i.reasons.join("; ")}`, entryId: null };
        const entry: HistoryEntry = {
          id: nextEntryId++, batchId, deletedAt: now, path: i.path, allocated: i.allocated,
          projectPath: i.projectPath, projectLabel: i.projectLabel, restoredAt: null,
        };
        history.push(entry);
        bin.push({
          id: `bin-c${entry.id}`, name: i.name, originalPath: i.path, deletedAt: now, kind: i.kind,
          size: i.allocated, fromDevDisk: true,
        });
        return { path: i.path, ok: true, error: null, entryId: entry.id };
      });
      // Offers are computed before the removal, while the project data is still there.
      const offers = new Map<string, ReinstallOffer>();
      todo.forEach((i) => {
        const o = offerFor(i);
        if (o) offers.set(o.projectPath, o);
      });
      applyRemoval(todo);
      cleaning = false;
      const d: CleanupDone = {
        batchId,
        freed: todo.reduce((a, i) => a + i.allocated, 0),
        items: results,
        reinstall: [...offers.values()],
      };
      cleanupDoneL.forEach((cb) => cb(d));
    }, 100);
    return batchId;
  },
  async listHistory() {
    await delay(60);
    return [...history].sort((a, b) => b.deletedAt - a.deletedAt || b.id - a.id).map((e) => ({ ...e }));
  },
  async restoreEntries(ids) {
    await delay(300);
    const now = Date.now();
    return ids.map((id): RestoreResult => {
      const e = history.find((x) => x.id === id);
      if (!e) return { id, ok: false, error: "unknown history entry" };
      if (e.restoredAt !== null) return { id, ok: false, error: "already put back" };
      if (now - e.deletedAt > DAY) return { id, ok: false, error: "no longer in the Recycle Bin" };
      e.restoredAt = now;
      return { id, ok: true, error: null };
    });
  },
  async startReinstall(projectPath) {
    if (reinstalling.has(projectPath)) throw "A reinstall is already running for this project.";
    reinstalling.add(projectPath);
    REINSTALL_LINES.forEach(([stream, line], i) => {
      setTimeout(() => reinstallOutL.forEach((cb) => cb({ projectPath, stream, line })), 150 + (i * 1800) / REINSTALL_LINES.length);
    });
    setTimeout(() => {
      reinstalling.delete(projectPath);
      reinstallDoneL.forEach((cb) => cb({ projectPath, success: true, exitCode: 0 }));
    }, 2100);
  },
  onCleanupProgress: (cb) => subscribe(cleanupProgressL, cb),
  onCleanupDone: (cb) => subscribe(cleanupDoneL, cb),
  onReinstallOutput: (cb) => subscribe(reinstallOutL, cb),
  onReinstallDone: (cb) => subscribe(reinstallDoneL, cb),
  async startDuplicates() {
    if (!summary) throw "no scan yet";
    if (timer !== null) throw "a scan is running, wait for it to finish";
    if (cleaning) throw "a cleanup is running, wait for it to finish";
    if (dupTimer !== null) throw "a duplicate search is already running";
    const scannedAt = summary.scannedAt;
    const candidates = files.filter((f) => f.size >= MB);
    const groups = dupGroups();
    const partialTotal = candidates.length;
    const fullFiles = groups.flat();
    const fullBytes = fullFiles.reduce((a, f) => a + f.size, 0);
    const start = Date.now();
    dupTimer = setInterval(() => {
      const t = Math.min(1, (Date.now() - start) / DUP_MS);
      if (t < 1) {
        const p: DupProgress =
          t < 0.4
            ? { phase: "partial", filesDone: Math.round((partialTotal * t) / 0.4), filesTotal: partialTotal, bytesDone: Math.round((partialTotal * MB * t) / 0.4), bytesTotal: partialTotal * MB }
            : { phase: "full", filesDone: Math.round((fullFiles.length * (t - 0.4)) / 0.6), filesTotal: fullFiles.length, bytesDone: Math.round((fullBytes * (t - 0.4)) / 0.6), bytesTotal: fullBytes };
        dupProgressL.forEach((cb) => cb(p));
        return;
      }
      if (dupTimer !== null) clearInterval(dupTimer);
      dupTimer = null;
      dupFor = { scannedAt, foundAt: Date.now() };
      dupDoneL.forEach((cb) => cb(null));
    }, 100);
  },
  async cancelDuplicates() {
    stopDuplicates("duplicate search cancelled");
  },
  async getDuplicates(query, limit): Promise<Duplicates | null> {
    await delay(80);
    if (!dupFor || !summary || dupFor.scannedAt !== summary.scannedAt) return null;
    const f = parse(query);
    const now = Date.now();
    const all: DuplicateGroup[] = dupGroups()
      .filter((g) => g.some((x) => matches(f, x.name, x.allocated, x.modified, now)))
      .map((g, i) => ({
        id: i,
        size: g[0].size,
        hash: (g[0].size * 2654435761).toString(16).padStart(64, "0").slice(-64),
        reclaimable: g[0].allocated * (g.length - 1),
        files: [...g].sort((a, b) => (a.modified ?? Infinity) - (b.modified ?? Infinity) || a.path.localeCompare(b.path)),
      }))
      .sort((a, b) => b.reclaimable - a.reclaimable);
    return {
      groups: all.slice(0, limit),
      totalGroups: all.length,
      totalReclaimable: all.reduce((a, g) => a + g.reclaimable, 0),
      issues: [
        { path: `${summary.root}\\personal\\downloads\\locked.vhdx`, message: "The process cannot access the file because it is being used by another process. (os error 32)" },
      ],
      issueCount: 1,
      hardLinked: 4,
      elapsedMs: DUP_MS,
      foundAt: dupFor.foundAt,
    };
  },
  onDuplicatesProgress: (cb) => subscribe(dupProgressL, cb),
  onDuplicatesDone: (cb) => subscribe(dupDoneL, () => cb()),
  onDuplicatesError: (cb) => subscribe(dupErrorL, cb),
  async startScan(root) {
    boot();
    stopDuplicates("duplicate search cancelled");
    if (bootTimer !== null) {
      clearTimeout(bootTimer);
      bootTimer = null;
    }
    if (timer !== null) throw "A scan is already running.";
    if (!root.trim()) throw "The path is not a folder.";
    const start = Date.now();
    const base = root.replace(/[\\/]+$/, "");
    const builtProjects = buildProjects(base, start);
    const builtFiles = withCopies(buildFiles(base, start), base);
    const totalAlloc =
      builtFiles.reduce((a, f) => a + f.allocated, 0) + builtProjects.reduce((a, p) => a + p.totalAllocated, 0);
    const totalFiles = 184_320;
    const totalDirs = 21_450;
    const duration = 2000;
    projects = [];
    files = [];
    summary = null;
    tree = null;
    volume = null;
    timer = setInterval(() => {
      const t = Math.min(1, (Date.now() - start) / duration);
      if (t < 1) {
        const p: ScanProgress = {
          files: Math.round(totalFiles * t),
          dirs: Math.round(totalDirs * t),
          bytes: Math.round(totalAlloc * t),
          currentPath: builtFiles[Math.floor(t * (builtFiles.length - 1))].path,
        };
        progressL.forEach((cb) => cb(p));
        return;
      }
      if (timer !== null) clearInterval(timer);
      timer = null;
      projects = builtProjects;
      files = builtFiles;
      tree = buildTree(root.trim(), builtProjects, new Map(SEEDS.map((x) => [x.name, x.sourceMb])));
      volume = mockVolume(root);
      const s: ScanSummary = {
        root,
        files: totalFiles,
        dirs: totalDirs,
        allocated: totalAlloc,
        size: Math.round(totalAlloc * 0.97),
        elapsedMs: Date.now() - start,
        issues: 3,
        projects: builtProjects.length,
        reclaimable: builtProjects.reduce((a, p) => a + p.reclaimable, 0),
        scannedAt: Date.now(),
      };
      summary = s;
      doneL.forEach((cb) => cb(s));
    }, 100);
  },
  async cancelScan() {
    if (timer === null) return;
    clearInterval(timer);
    timer = null;
    errorL.forEach((cb) => cb({ message: "Scan cancelled.", cancelled: true }));
  },
  async getSummary() {
    boot();
    return summary;
  },
  async listProjects(query) {
    await delay(40);
    const f = parse(query);
    const now = Date.now();
    return projects.filter((p) => matches(f, p.name, p.totalAllocated, p.lastActivity, now));
  },
  async listFiles(query, sort: FileSort, descending, limit) {
    await delay(60);
    const f = parse(query);
    const now = Date.now();
    const list = files.filter((x) => matches(f, x.name, x.allocated, x.modified, now));
    const dir = descending ? -1 : 1;
    list.sort((a, b) => {
      if (sort === "name") return dir * a.name.localeCompare(b.name);
      if (sort === "modified") return dir * ((a.modified ?? 0) - (b.modified ?? 0));
      return dir * (a[sort] - b[sort]);
    });
    return list.slice(0, limit);
  },
  async getVolume() {
    await delay(30);
    return volume;
  },
  async getTreemap(path, depth) {
    await delay(80);
    return mockTreemap(tree, path, depth);
  },
  onProgress: (cb) => subscribe(progressL, cb),
  onDone: (cb) => subscribe(doneL, cb),
  onError: (cb) => subscribe(errorL, cb),
};

