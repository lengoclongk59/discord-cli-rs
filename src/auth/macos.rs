//! macOS token auto-discovery.
//!
//! README declares macOS/Linux token discovery a non-goal for the MVP. The
//! previous implementation pattern-matched a fixed-length regex against
//! Local Storage files; modern Discord/Chrome encrypt the token via the
//! Keychain on macOS, so the plaintext scan recovers nothing useful and
//! the regex itself missed valid tokens. Until a Keychain-aware port
//! lands, fail clearly and tell the user to use `--token` or
//! `DISCORD_TOKEN`.

use anyhow::{anyhow, Result};

use super::DiscoveredToken;

pub async fn find_and_save_token() -> Result<DiscoveredToken> {
    Err(anyhow!(
        "auth --save is not supported on macOS. Set $DISCORD_TOKEN or pass --token <T>."
    ))
}
