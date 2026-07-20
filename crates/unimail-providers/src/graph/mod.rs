//! Microsoft public-client OAuth and Graph mail adapter.

mod client;
mod config;
mod credential;
mod dto;
mod oauth;
mod provider;
mod registry;

pub use config::GraphConfig;
pub use oauth::GraphAuthenticator;
pub use provider::GraphProvider;
pub use registry::GraphAccountRegistry;
