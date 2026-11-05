//! The `BranchBooks` core: types, parser, loader, ledger, budget and write path.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod book;
pub mod budget;
pub mod date;
pub mod diff;
pub mod ledger;
pub mod loader;
pub mod money;
pub mod parser;
pub mod reports;
pub mod types;
pub mod write;

/// The core's version, from the workspace's `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
