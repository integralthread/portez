//! `portez` hands out stable development port numbers.
//!
//! A registry file (TOML) maps a project directory and a name to a port.
//! The first request for a `(directory, name)` pair allocates the lowest
//! free port at or above the configured start; every later request returns
//! the same number.

#![forbid(unsafe_code)]

pub mod cli;
pub mod registry;

pub use registry::{Assignment, Registry, RegistryError};
