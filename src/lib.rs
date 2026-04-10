//! flotop — Agentless multi-host TUI for triaging a fleet of VPSes when something breaks at 2am.
//!
//! Formerly sitrep. Renamed for crates.io availability + the multi-host wedge.
//!
//! This library exposes the core modules for use by the binary and by tests.

pub mod cli;
pub mod model;
pub mod view;
pub mod layout;
pub mod controller;
pub mod collectors;
pub mod docker;
pub mod docker_controller;
pub mod swarm;
pub mod swarm_controller;
pub mod swarm_helpers;
pub mod remote_docker;
pub mod remote_host;
pub mod remote_swarm;
pub mod snapshot;
pub mod app;
