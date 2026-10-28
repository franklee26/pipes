# Pipes desktop

The Tauri 2 / React frontend for the shared Rust manifest registry.

Run `npm run dev` from the repository root to open the desktop app. See the
[project README](../../README.md) for prerequisites, CLI commands, storage,
manifest rules, and validation.

`npm run dev --workspace apps/desktop` starts only the Vite frontend. A regular
browser cannot access native registry operations and reports that explicitly.
