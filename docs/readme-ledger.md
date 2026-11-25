# README fact ledger

Built by the project-readme skill on 2026-11-22. Every claim the README makes comes from this table.

| Fact | Status | Source |
|---|---|---|
| Name is Ballast; logo and lockups in `logos/final/` | Verified | `logos/README.md`, `src-tauri/tauri.conf.json` |
| Windows desktop app: Tauri 2, Rust backend, React + TypeScript (Vite) frontend | Verified | `Cargo.toml`, `package.json`, `PROJECT.md` §3 |
| Recognizes dev projects and labels them by framework ("React project: my-shop") | Verified | `PROJECT.md` §2, `ProjectsTab.tsx`, `ProjectCard.tsx` |
| Knows which folders can be rebuilt (`node_modules`, `target`, `.next`, `vendor`, ...); git-tracked folders are never offered | Verified | `PROJECT.md` §3 |
| Cleanup: preview first, Recycle Bin by default, every cleanup logged with "Put back" | Verified | `PROJECT.md` §3-4, `CleanupDialog.tsx`, `HistoryTab.tsx` |
| "Reinstall dependencies" after deleting `node_modules` | Verified | `PROJECT.md` §4, `ReinstallLog.tsx` |
| Tabs: Projects, Biggest files, Duplicates, Map (treemap), History, Apps, Dev tools | Verified | `src/components/`, app screenshot |
| Search with wildcards and AND terms (`*.mp3 dance`), size and time filters | Verified | `PROJECT.md` §4, `SearchBar.tsx` |
| Content-based duplicates (size, then partial hash, then full hash), files of 1 MB or more | Verified | `PROJECT.md` §3 |
| Fast scan (admin): reads the NTFS MFT through a UAC-elevated helper; all of C: (1.9 M files) in 16.6 s | Verified | `PROJECT.md` §3 (measured 2026-11-21) |
| Size on disk is the main number (cloud-only OneDrive files count as 0) | Verified | `PROJECT.md` §3 |
| Apps tab (desktop + Store apps, read-only) and Dev tools tab (languages with every version, tools with their data folders) | Verified | `PROJECT.md` §3-4 |
| Rust edition 2024 (Rust 1.85 or newer) | Verified | `crates/*/Cargo.toml` |
| CLI usage: `devdisk scan <path> [--top N] [--mft]` (top defaults to 15) | Verified | `crates/devdisk-cli/src/main.rs` |
| Develop: `npm install`, then `npm run tauri dev` | Verified | `package.json` scripts, current README |
| Browser preview with mock data: `npm run dev` (port 1420) | Verified | `package.json`, `src/api/mock.ts`, `.claude/launch.json` |
| Closest tools: kondo, npkill, WizTree, WinDirStat, Czkawka; the difference is framework-aware recognition + rebuild-safe cleanup + undo | Verified | `PROJECT.md` §5 |
| Repo is private, `ChrApostolidis/disk-cleaner`, no releases, no CI, no LICENSE | Verified | `gh repo view`, `gh release list`, root listing |
| Built in eight days (2026-11-15 to 2026-11-22), starting from an empty Tauri scaffold | Verified | `git log` |
| Why I Built It and The Problem It Solves come right after the screenshot, before any setup, for non-technical readers | Confirmed | User, 2026-11-22 |
| Built for the user's own machine first | Confirmed | User, 2026-11-22 |
| The moment: a weekend project, started after seeing the disk almost full | Confirmed | User, 2026-11-22 |
| Good alternatives exist; Ballast is the developer-oriented one. The pains are the gaps in generic tools (`PROJECT.md` §1, §5) | Confirmed | User, 2026-11-22 |
| Why I built it: same as the moment above. No learning goals given, so that part is left out | Confirmed | User, 2026-11-22 |
| Decisions to highlight: the skill's pick from `PROJECT.md` §3 | Confirmed | User, 2026-11-22 ("whatever you think") |
| Roadmap: nothing planned, so no Roadmap section | Confirmed | User, 2026-11-22 |
| No installer on Releases, so Quick Start is build-from-source only | Confirmed | User, 2026-11-22 |
| License: MIT. A `LICENSE` file is added with the README | Confirmed | User, 2026-11-22 |
| Repo stays `ChrApostolidis/disk-cleaner` and private, so only static badges | Confirmed | User, 2026-11-22 |
