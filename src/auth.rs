//! Token auto-discovery (Windows DPAPI + LevelDB brute-force scan).
//!
//! On non-Windows platforms, `find_and_save_token` will fall back to platform-specific
//! scanners without decryption.

#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;

#[derive(Debug)]
pub struct DiscoveredToken {
    pub token: String,
    pub source: String,
    pub username: String,
}

/// Scan local Discord storage, decrypt candidate tokens via DPAPI (Windows only), validate
/// each via `GET /users/@me`, and return the first valid one.
#[cfg(windows)]
pub async fn find_and_save_token() -> anyhow::Result<DiscoveredToken> {
    windows::find_and_save_token().await
}

#[cfg(target_os = "macos")]
pub async fn find_and_save_token() -> anyhow::Result<DiscoveredToken> {
    macos::find_and_save_token().await
}

#[cfg(target_os = "linux")]
pub async fn find_and_save_token() -> anyhow::Result<DiscoveredToken> {
    linux::find_and_save_token().await
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub async fn find_and_save_token() -> anyhow::Result<DiscoveredToken> {
    anyhow::bail!(
        "auth --save is currently unsupported on this platform. \
         Please set $DISCORD_TOKEN or pass --token <T>."
    )
}
