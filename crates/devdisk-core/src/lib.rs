//! Core logic for DevDisk. Has no Tauri dependency so it can be tested
//! on its own and driven by the CLI or the desktop app.

pub mod detect;
pub mod index;
pub mod projects;
pub mod rules;
pub mod scanner;
pub mod search;
