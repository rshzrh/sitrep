//! Sitrep — A real-time terminal diagnostic tool for server triage.
//!
//! This library exposes the core modules for use by the binary and by tests.

pub mod model;
pub mod view;
pub mod layout;
pub mod controller;
pub mod collectors;
pub mod docker;
pub mod docker_controller;
pub mod swarm;
pub mod swarm_controller;
pub mod app;
