// Browser-only stand-in for the Rust backend so the UI can be developed with `npm run dev`.
import type {
  DevDiskApi,
  FileEntry,
  FileSort,
  Project,
  ScanError,
  ScanProgress,
  ScanSummary,
} from "./types";

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

let projects: Project[] = [];
let files: FileEntry[] = [];
let summary: ScanSummary | null = null;
let timer: ReturnType<typeof setInterval> | null = null;

function subscribe<T>(set: Listeners<T>, cb: (v: T) => void): Promise<() => void> {
  set.add(cb);
  return Promise.resolve(() => {
    set.delete(cb);
  });
}
const delay = (ms: number) => new Promise<void>((res) => setTimeout(res, ms));

export const mockApi: DevDiskApi = {
  async startScan(root) {
    if (timer !== null) throw "A scan is already running.";
    if (!root.trim()) throw "The path is not a folder.";
    const start = Date.now();
    const base = root.replace(/[\\/]+$/, "");
    const builtProjects = buildProjects(base, start);
    const builtFiles = buildFiles(base, start);
    const totalAlloc =
      builtFiles.reduce((a, f) => a + f.allocated, 0) + builtProjects.reduce((a, p) => a + p.totalAllocated, 0);
    const totalFiles = 184_320;
    const totalDirs = 21_450;
    const duration = 2000;
    projects = [];
    files = [];
    summary = null;
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
  onProgress: (cb) => subscribe(progressL, cb),
  onDone: (cb) => subscribe(doneL, cb),
  onError: (cb) => subscribe(errorL, cb),
};

export const MOCK_DEFAULT_ROOT = "C:\\Users\\you\\Projects";
