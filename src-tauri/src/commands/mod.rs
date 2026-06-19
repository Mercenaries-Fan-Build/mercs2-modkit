//! Tauri command handlers exposed to the Vue frontend via `invoke`.

pub mod asset_catalog;
pub mod conflict_resolver;
pub mod deploy;
pub mod game;
pub mod installer;
pub mod launch;
pub mod logprobe;
pub mod mod_loader;
pub mod paths;
pub mod registry;
pub mod setup;
pub mod updates;
pub mod validator;
pub mod wad_builder;
