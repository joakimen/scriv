//! Command implementations — the imperative shell.
//!
//! Each submodule mirrors a top-level command in the CLI. Functions take a
//! [`crate::Ctx`] and perform the filesystem and interactive I/O; all decision
//! logic they rely on lives in the pure core modules.

pub mod branch;
pub mod config;
pub mod edit;
pub mod file;
pub mod history;
pub mod pr;
pub mod proc;
pub mod repo;
