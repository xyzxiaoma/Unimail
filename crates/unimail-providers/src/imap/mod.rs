mod auth;
mod credential;
mod cursor;
mod preset;
mod provider;
mod registry;
mod session;
mod smtp;
mod tls;

#[cfg(test)]
mod test_support;

pub use auth::ImapAuthenticator;
pub use preset::{ImapSmtpPreset, NETEASE_PRESET, QQ_PRESET, preset_for};
pub use provider::ImapProvider;
pub use registry::ImapAccountRegistry;
