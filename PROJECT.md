# DevDisk (working title) — Developer-focused disk cleaner for Windows

> "DevDisk" is a placeholder name. Rename freely.

This file is the single source of truth for the project: what we are building, the decisions already made, and how to build it. Read it fully before writing code. If a decision here needs to change, change it here too.

## 1. The problem

Windows storage management is poor and third-party cleaners don't understand developers. Developers create many projects, most are abandoned early, and each one keeps gigabytes of regenerable folders (`node_modules`, `.next`, `target`, `vendor`, ...). Generic analyzers show big folders but can't say "this is a React project, its `node_modules` is 600 MB and can be regenerated".

## 2. What the app is

A Windows desktop app that scans the disk and shows where space goes, with three things generic tools lack:

1. **Project recognition**: it recognizes dev projects and labels them by framework, e.g. "React project: my-shop".
2. **Regenerable-folder awareness**: it knows which folders inside a project can be safely recreated.
3. **Safe cleanup with undo**: preview first, Recycle Bin by default, every cleanup logged and restorable.

It is also a general disk analyzer (treemap, biggest files, duplicates), not only a dev-junk finder.

### Non-goals (for now)
- No cross-platform support (Windows first; keep code portable where free).
- No cloud-drive scanning, no file shredding / free-space wiping.
- No scan history / trends ("what grew"), no scan cache, no extra junk finders (empty folders, broken files). These were discussed and deliberately left out; they can come later.
- No auto-delete. Nothing is ever removed without the user confirming a preview.

## 3. Decisions made

| Decision | Choice | Why / tradeoff |
|---|---|---|
| App type | Desktop app (not a web app) | A browser sandbox cannot read the whole disk or delete files. |
| Wrapper | **Tauri** | Small installer (~10 MB), Rust backend is fast for scanning. Tradeoff: Rust learning curve. (Electron was the faster-to-start alternative.) |
| Backend | **Rust** | Parallel directory walking, safe, fast. |
| Frontend | **React + TypeScript** (Vite) | Already known; best ecosystem for treemap/table libraries. |
| Scanner design | Behind one interface (trait) with two implementations | v1 normal scanner works everywhere; v2 adds NTFS MFT scanner without touching the rest of the app. |
| Delete behavior | Recycle Bin by default, never permanent unless the user explicitly chooses | Safety. |
| Duplicates | Content-based (size → partial hash → full hash) | Name-only matching gives false positives. |
| Rules | Data file (JSON), not hard-coded | Adding a framework/ecosystem should not need new code. |

## 4. Features

Phase numbers refer to section 8.

| Area | Feature | Phase |
|---|---|---|
| Project recognition | Framework labels from `package.json` / `composer.json` etc. | 1 |
| Project recognition | Project cards: name, framework, size, last modified, reclaimable size | 2 |
| Scanning | Standard parallel scanner (skips symlinks/junctions, survives access-denied) | 1 |
| Scanning | Admin fast mode: read the NTFS Master File Table directly | 6 |
| Visualization | Treemap of a drive/folder | 3 |
| Visualization | Treemap colored by framework | 3 |
| Visualization | Free-space block in the treemap | 3 |
| Search / filters | Flexible search: wildcards, AND terms (e.g. `*.mp3 dance`) | 2 |
| Search / filters | Size filters (e.g. > 500 MB) | 2 |
| Search / filters | Time filters (e.g. untouched for 90 days) | 2 |
| Biggest files | Sortable biggest-files list sharing the search bar | 2 |
| Duplicates | Content-based duplicate finder | 5 |
| Cleaning | Preview before delete (what goes, how much space freed) | 4 |
| Cleaning | Delete to Recycle Bin by default | 4 |
| Cleaning | Logged restore ("Put back") for every cleanup | 4 |
| Cleaning | "Reinstall dependencies" button after deleting `node_modules` | 4 |

## 5. Landscape (what already exists, and where we differ)

- **kondo** (Rust, MIT): CLI + GUI, 20+ project types, age filter, dry run. Detects by ecosystem (Node, Cargo...), not by framework. Closest competitor. Read its detection code before claiming framework recognition is unique.
- **npkill**: finds/deletes `node_modules` (and other target names), last-modified info, multi-select, dry run, JSON output.
- **dust (ariefsn)**: detects 11 project kinds, per-project size, stale projects, skips dirty git repos by default.
- **BleachBit 6**: general cleaner that added deep-scan options for `node_modules` and `venv`.
- **WizTree**: very fast via direct MFT reading; wildcard/AND search, relative time filters, CSV export. Duplicate detection by name/size/date only (not content).
- **WinDirStat / SpaceSniffer / SpaceBrowser**: treemaps; SpaceBrowser has a free-space node.
- **Czkawka (Rust)**: duplicates by name/size/hash/first-MB hash, plus other finders.
- **DiskFox (macOS)**: Trash-first cleanup with history and "Put Back".

**Our difference:** framework-aware project recognition + regenerable-folder safety + undo, inside a general analyzer, on Windows.

## 6. Architecture

```
React UI (TypeScript)
   │  invoke(command) / listen(event)
Tauri command layer  (src-tauri)
   │
devdisk-core  (Rust library crate, no Tauri dependency)
   ├── scanner/      Scanner trait + WalkScanner (v1) + MftScanner (v2)
   ├── index/        in-memory tree of folders with aggregated sizes
   ├── detect/       project + framework detection, driven by rules.json
   ├── search/       wildcard/AND search, size and time filters
   ├── duplicates/   size → partial hash → full hash
   ├── cleanup/      preview, recycle-bin delete, restore log (SQLite)
   └── rules/        rules.json loading + validation
```

Keep `devdisk-core` independent of Tauri so it can be tested and run as a CLI in Phase 1.

### 6.1 Scanner interface
- One trait, e.g. `Scanner::scan(root, options, progress_sink) -> ScanResult`.
- `WalkScanner` (v1): parallel walk with `jwalk`/`rayon`.
- `MftScanner` (v2): reads the NTFS MFT. Needs administrator rights (UAC prompt) and NTFS only. Gives names, sizes, timestamps. It does not read file contents, so duplicate hashing still opens files.
- Must never follow symlinks or junctions (avoids double counting and loops), must handle long paths, and must skip and record access-denied folders instead of failing.
- Stream progress events (files scanned, current path) to the UI. Do not return one big result at the end or the UI will freeze.

### 6.2 Data flow to the UI
- Do **not** send the whole file tree to React. Keep the tree in Rust. Send aggregated folders to a limited depth and load children lazily when the user drills down. The treemap and lists query by path.
- Large lists (biggest files, duplicates) must be virtualized in the UI.

### 6.3 Project and framework detection
- A project root is a folder containing a marker file (`package.json`, `composer.json`, `Cargo.toml`, `pom.xml`, `build.gradle`, `pyproject.toml`, `requirements.txt`, ...).
- Do **not** treat `package.json` files inside `node_modules`, `vendor`, `.git` or build output as projects. Stop project detection when entering a known regenerable folder.
- Monorepos: a root with workspaces can contain several packages; report the root and (optionally) its packages.
- Framework from dependencies, first match wins (starting list, refine as we test):

| Marker | Label |
|---|---|
| `next` in dependencies | Next.js |
| `@angular/core` | Angular |
| `nuxt` | Nuxt |
| `@sveltejs/kit` | SvelteKit |
| `astro` | Astro |
| `@remix-run/*` | Remix |
| `expo` or `react-native` | React Native |
| `@nestjs/core` | NestJS |
| `vue` | Vue |
| `react` + `vite` | React (Vite) |
| `react` | React |
| `express` | Node.js (Express) |
| anything else with `package.json` | Node.js |
| `laravel/framework` in `composer.json` | Laravel |
| `Cargo.toml` | Rust |
| `spring-boot` in `pom.xml` / Gradle | Spring Boot |

- The label is shown as "React project: <folder name>", with the version when known.

### 6.4 Rules file (`rules.json`)
Maps markers to regenerable folders. Example shape:

```json
{
  "ecosystems": [
    {
      "id": "node",
      "markers": ["package.json"],
      "lockfiles": ["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "bun.lockb"],
      "regenerable": ["node_modules", ".next", ".nuxt", ".turbo", "dist", ".svelte-kit"],
      "reinstall": { "npm": "npm ci", "pnpm": "pnpm install --frozen-lockfile", "yarn": "yarn install --frozen-lockfile" }
    },
    { "id": "rust", "markers": ["Cargo.toml"], "regenerable": ["target"], "reinstall": null },
    { "id": "php",  "markers": ["composer.json"], "regenerable": ["vendor"], "lockfiles": ["composer.lock"], "reinstall": { "composer": "composer install" } }
  ],
  "frameworks": [
    { "label": "Next.js", "ecosystem": "node", "dependency": "next" }
  ]
}
```

Safety rule: a folder is only offered as regenerable if its project has a lockfile, or the preview clearly warns that reinstalling may pick different versions.

### 6.5 Search and filters
- One search bar shared by the biggest-files list and the project list.
- Wildcards (`*`, `?`), space-separated terms act as AND (`*.mp3 dance`).
- Size filters (`> 500 MB`) and time filters ("not modified in 90 days", "modified in the last hour").
- Presets: "older than 90 days", "bigger than 500 MB".

### 6.6 Duplicates
1. Group files by exact size (ignore tiny files below a threshold).
2. Within a group, hash the first 1 MB.
3. For survivors, hash the full file (blake3).
4. Show groups; the user picks which copies to remove; goes through the same preview and delete flow.

### 6.7 Cleanup, preview and restore
- Every delete action first shows a preview: items, per-item size, total space freed, and whether each item is a regenerable folder.
- Delete = move to Recycle Bin (`trash` crate). Never permanent by default.
- Every cleanup is written to a SQLite log: id, timestamp, original path, size, project, framework, action.
- "Put back" restores from the Recycle Bin by original path and marks the log entry restored. If the item is gone from the bin, say so.
- After deleting a `node_modules`, offer "Reinstall dependencies" using the package manager detected from the lockfile.
- Refuse system/protected paths (Windows, Program Files, the app's own data) outright.

### 6.8 Treemap
- Block size = folder size. Color = framework when the folder is a recognized project, otherwise by file type or neutral.
- Include a **free-space block** for the volume.
- Click to drill in; breadcrumb to go back.
- Library options: `d3-hierarchy` for layout (custom SVG/canvas) or ECharts treemap.

## 7. Suggested libraries (verify current versions when adding)

Rust: `jwalk`, `rayon`, `trash`, `rusqlite`, `serde` / `serde_json`, `blake3`, `globset` (or similar) for wildcards, `ntfs` crate (evaluate for v2).
Frontend: React, TypeScript, Vite, `d3-hierarchy` or ECharts, TanStack Table + a virtualization library, Tailwind (optional).

## 8. Build phases

| Phase | Goal | Done when |
|---|---|---|
| 1 | `devdisk-core` + CLI: scanner + project/framework detection | CLI prints the biggest folders and a framework label per project, on a real dev folder |
| 2 | Tauri + React UI: project cards, biggest files, search and filters | Can scan a folder in the app, see progress, search and filter results |
| 3 | Treemap, framework colors, free-space block | Clicking a block drills into the folder |
| 4 | Safe delete: preview, Recycle Bin, restore log, reinstall button | A deleted folder can be put back from the app |
| 5 | Duplicate finder | Content-based duplicates found and cleanable with preview |
| 6 | Admin MFT fast mode | NTFS scan runs through `MftScanner` behind the same trait |

Always keep the app working at the end of each phase. Build Phase 1 before any UI.

## 9. Windows gotchas to handle
- Junctions and symlinks: never follow.
- Long paths (> 260 chars) and access-denied folders.
- Hard links can double count sizes.
- OneDrive "cloud-only" files appear in folders but take no local space.
- WSL2 `.vhdx` and Docker Desktop disks are often the largest hidden items. Not in v1 scope, but keep the rules file able to describe known locations.
- `node_modules` of installed apps must not be flagged as deletable project junk (npkill warns about this).

## 10. Open questions
- App name.
- Skip or warn on projects with uncommitted git changes? (dust skips them by default.) Not decided.
- Detect "abandoned / never started" projects (few commits, untouched template)? Suggested earlier, not selected yet.
- Which folders are scanned by default (whole drive vs. user-chosen project roots)?

## 11. Conventions for working on this repo
- Small commits. Commit messages: short, specific, imperative mood (e.g. "Add framework detection for Next.js"). No AI attribution or co-author lines in commits, PR descriptions or code comments.
- Core logic lives in `devdisk-core` with unit tests (use temporary folders with fake `package.json` files to test detection). The Tauri layer stays thin.
- Never write code that deletes files without going through the preview and Recycle Bin flow.
- Update this file when a decision changes.
