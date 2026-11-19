//! Core logic for DevDisk. Has no Tauri dependency so it can be tested
//! on its own and driven by the CLI or the desktop app.

pub mod apps;
pub mod bin;
pub mod cleanup;
pub mod detect;
pub mod index;
pub mod projects;
#[cfg(windows)]
mod reg;
pub mod rules;
pub mod scanner;
pub mod search;
pub mod snapshot;
pub mod treemap;
pub mod volume;
