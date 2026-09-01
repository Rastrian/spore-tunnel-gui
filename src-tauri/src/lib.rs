//! Spore Tunnel GUI library target.
//!
//! The protocol core and app configuration live here so they can be
//! unit-tested and reused independently of the Tauri application shell
//! in `main.rs`.

pub mod config;
pub mod discover;
pub mod tunnel;
