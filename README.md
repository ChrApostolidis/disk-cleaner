# DevDisk

Developer-focused disk cleaner for Windows. Finds dev projects, labels them by framework, and safely cleans regenerable folders (`node_modules`, `target`, `vendor`, ...) with preview, Recycle Bin and undo.

See [PROJECT.md](PROJECT.md) for the full plan and decisions.

## Stack

- Tauri 2 (Rust backend) + React + TypeScript (Vite)

## Develop

```bash
npm install
npm run tauri dev
```
