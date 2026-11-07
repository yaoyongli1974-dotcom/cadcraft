//! AICAD AI assistant crate (layer 4, `engine -> ai`).
//!
//! Responsibilities:
//! - snapshot the drawing context the AI window sees (`assistant::context`);
//! - turn a natural-language request into a command draft, either through a
//!   configurable OpenAI-compatible endpoint (`openai`, native builds only) or
//!   the bundled offline rule engine (`rules`);
//! - apply or dry-run a draft against a [`Session`] as a single undo group
//!   (`executor`).
//!
//! No UI, no async runtime, no panics: errors are `String`s, HTTP lives behind
//! `cfg(not(target_arch = "wasm32"))` and runs on a caller-provided thread.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod assistant;
pub mod executor;
#[cfg(not(target_arch = "wasm32"))]
pub mod openai;
pub mod provider;
pub mod rules;

#[cfg(test)]
mod tests;

pub use assistant::{ChatTurn, Draft, Role, context, suggest};
pub use executor::{LineKind, LineReport, active_lines, apply, dry_run};
pub use provider::{AiMode, AiSettings, OpenAiSettings};
