// Core business logic - always available
pub mod core;
pub mod types;

// Infrastructure modules - needed for core functionality
pub mod database;
pub mod error;
pub mod events;
pub mod observability;
pub mod utils;

// API modules - only available when "api" feature is enabled
#[cfg(feature = "api")]
pub mod api;

#[cfg(feature = "api")]
pub mod auth;

#[cfg(feature = "api")]
pub mod config;

#[cfg(feature = "api")]
pub mod health;

#[cfg(feature = "api")]
pub mod metrics;

#[cfg(feature = "api")]
pub mod state;

#[cfg(feature = "api")]
pub mod webhooks;

// Note: Legacy re-exports removed in favor of explicit core:: imports
// Main library exports
pub use core::multimint::MultiMint;
pub use core::FmcdCore;

#[cfg(feature = "api")]
pub use api::rest as router;
// Only export types when API feature is enabled since they depend on API types
#[cfg(feature = "api")]
pub use types::*;
