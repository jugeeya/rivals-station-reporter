//! The UI-free application engine — a direct port of the Tauri shell's
//! backend (engine.rs / config.rs / hub_glue.rs), with Tauri's event system
//! replaced by a plain callback (`EngineInner::set_emitter`) and the command
//! surface exposed as ordinary functions (`commands`). station-core still
//! holds all the domain logic; this layer owns the loop thread, the config
//! file, and hub wiring.

pub mod commands;
pub mod config;
pub mod core;
pub mod hub_glue;

pub use core::{start, Engine, EngineInner};
