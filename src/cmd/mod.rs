//! Command implementations — the imperative shell.
//!
//! Each submodule mirrors a top-level noun in the CLI. Functions take a
//! [`crate::Ctx`] and perform the filesystem and interactive I/O; all decision
//! logic they rely on lives in the pure core modules.

pub mod branch;
pub mod config;
pub mod file;
pub mod pr;
pub mod repo;
