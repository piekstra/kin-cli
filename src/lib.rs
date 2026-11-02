//! `kin` — read-only CLI for the Kin Insurance customer portal.
//!
//! The binary's modules, exposed as a library so the integration tests can
//! drive the parsers over fixtures.

pub mod bundle;
pub mod client;
pub mod config;
pub mod dto;
pub mod jar;
pub mod parse;
pub mod session;
