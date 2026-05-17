//! Shared types and logic for Agit.
//!
//! This crate is the home of everything that the CLI and the (future) server
//! need to agree on: the YAML schema, the policy model, the run state machine.
//! Keep it I/O-light and dependency-light so both surfaces stay cheap to build.

pub mod config;
pub mod history;
pub mod logs;
pub mod policy;
pub mod trace;
pub mod usage;
