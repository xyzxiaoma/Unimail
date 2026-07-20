//! Gmail OAuth and Gmail REST adapter.

mod client;
mod config;
mod credential;
mod dto;
mod oauth;
mod provider;
mod registry;

pub use config::GmailConfig;
pub use oauth::GmailAuthenticator;
pub use provider::GmailProvider;
pub use registry::GmailAccountRegistry;
