//! Windows token discovery for Discord (Electron + Chromium).
//!
//! ## Discord storage layout (verified empirically)
//!
//! Discord stores the user token in the LevelDB at
//! `%APPDATA%\Discord\Local Storage\leveldb\` under values prefixed with the
//! literal `dQw4w9WgXcQ:` followed by base64-encoded **AES-256-GCM**
//! ciphertext. The AES key is itself stored in
//! `%APPDATA%\Discord\Local State` as JSON
//! (`os_crypt.encrypted_key` — base64, with a literal `DPAPI` ASCII prefix,
//! then DPAPI-encrypted).
//!
//! ## Decryption pipeline
//!
//! 1. Read `Local State`, parse JSON, base64-decode `os_crypt.encrypted_key`,
//!    strip the 5-byte ASCII `DPAPI` prefix, DPAPI-decrypt → 32-byte AES key.
//! 2. For each `dQw4w9WgXcQ:<base64>` hit in LevelDB files:
//!    - base64-decode → bytes
//!    - first 3 bytes are the version (`v10` or `v11`); strip
//!    - next 12 bytes are the AES-GCM nonce
//!    - remaining bytes are ciphertext || 16-byte tag
//!    - AES-256-GCM decrypt → token plaintext
//! 3. Validate via `GET /users/@me`.
//!
//! ## Why a brute-force byte scan
//!
//! Off-the-shelf Rust LevelDB crates struggle with Chromium's variant of
//! the format. The token values are present in raw SST/log files and the
//! literal-prefix scan recovers them reliably across Discord versions.
//!
//! ⚠ This file is the SOLE sanctioned exception to the read-only HTTP
//! invariant: `validate_token` uses raw `reqwest` directly because it must
//! run *before* a token exists in the resolution chain. The
//! `tests/readonly.rs` test allowlists this file specifically.

use std::path::{Path, PathBuf};
use std::ptr;
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde::Deserialize;
use tracing::{debug, info};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
use zeroize::Zeroizing;

use super::DiscoveredToken;
use crate::config::looks_like_discord_token;

const TOKEN_PREFIX: &[u8] = b"dQw4w9WgXcQ:";
const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
const DPAPI_PREFIX: &[u8] = b"DPAPI";
const GCM_NONCE_LEN: usize = 12;
const VALIDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(15);
/// Cap each candidate blob to a sane ceiling. Real Discord token blobs are
/// well under 1KB.
const MAX_BLOB_BYTES: usize = 4096;
/// Cap DPAPI input size — Win32 takes a `u32` byte count and we want to
/// fail loud rather than silently truncate.
const MAX_DPAPI_INPUT: usize = u32::MAX as usize;

#[derive(Debug, Deserialize)]
struct MeMin {
    username: String,
    global_name: Option<String>,
}

pub async fn find_and_save_token() -> Result<DiscoveredToken> {
    let discord_dir = discord_app_dir().context("could not locate %APPDATA%\\Discord")?;
    let leveldb_dir = discord_dir.join("Local Storage").join("leveldb");
    let local_state = discord_dir.join("Local State");
    if !leveldb_dir.exists() {
        return Err(anyhow!(
            "Discord LevelDB directory not found at {}. Is the Discord desktop app installed?",
            leveldb_dir.display()
        ));
    }
    if !local_state.exists() {
        return Err(anyhow!(
            "Discord 'Local State' file not found at {}. The Chromium master key cannot be recovered.",
            local_state.display()
        ));
    }

    // Step 1 & 2 (CPU + sync I/O): recover master key, scan LevelDB,
    // AES-GCM decrypt candidate blobs. Done on a blocking thread so we
    // don't stall the tokio worker driving validation HTTP.
    let local_state_for_blocking = local_state.clone();
    let leveldb_dir_for_blocking = leveldb_dir.clone();
    let candidates = tokio::task::spawn_blocking(move || -> Result<Vec<Zeroizing<String>>> {
        let master_key = recover_master_key(&local_state_for_blocking).with_context(|| {
            format!(
                "failed to recover master key from {}",
                local_state_for_blocking.display()
            )
        })?;
        let blobs = scan_for_token_blobs(&leveldb_dir_for_blocking).with_context(|| {
            format!(
                "failed to read {} (close Discord and retry — LevelDB may be locked)",
                leveldb_dir_for_blocking.display()
            )
        })?;
        if blobs.is_empty() {
            return Ok(Vec::new());
        }
        info!(blob_count = blobs.len(), "decrypting candidate blobs via AES-GCM");

        let master_key = Zeroizing::new(master_key);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&master_key));
        let mut out: Vec<Zeroizing<String>> = Vec::new();
        for blob in blobs {
            if blob.len() > MAX_BLOB_BYTES {
                continue;
            }
            match aes_gcm_decrypt(&cipher, &blob) {
                Ok(plain) => {
                    let plain = Zeroizing::new(plain);
                    let candidate = match std::str::from_utf8(&plain) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if candidate.is_empty() || !looks_like_discord_token(candidate) {
                        continue;
                    }
                    out.push(Zeroizing::new(candidate.to_string()));
                }
                Err(e) => {
                    debug!(error = %e, "AES-GCM decrypt failed");
                }
            }
        }
        Ok(out)
    })
    .await
    .context("spawn_blocking for token scan panicked")??;

    if candidates.is_empty() {
        return Err(anyhow!(
            "No encrypted token blobs decrypted to a plausible Discord token in {}.\n\
             Make sure you're logged into the Discord desktop app.",
            leveldb_dir.display()
        ));
    }

    // Step 3 (async network I/O): validate each candidate against
    // `GET /users/@me`. First success wins.
    let mut last_err: Option<String> = None;
    for candidate in &candidates {
        match validate_token(candidate).await {
            Ok(me) => {
                return Ok(DiscoveredToken {
                    token: candidate.to_string(),
                    source: format!("{}", leveldb_dir.display()),
                    username: me.global_name.unwrap_or(me.username),
                });
            }
            Err(e) => {
                last_err = Some(format!("validate: {}", e));
            }
        }
    }
    Err(anyhow!(
        "All candidate tokens failed to validate. Last error: {}",
        last_err.unwrap_or_else(|| "(unknown)".to_string())
    ))
}

#[derive(Deserialize)]
struct LocalState {
    os_crypt: OsCrypt,
}
#[derive(Deserialize)]
struct OsCrypt {
    encrypted_key: String,
}

/// Recover the 32-byte AES-GCM master key from `%APPDATA%\Discord\Local State`.
fn recover_master_key(local_state_path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read_to_string(local_state_path)?;
    let parsed: LocalState = serde_json::from_str(&raw).context("Local State is not valid JSON")?;
    let key_b64 = parsed.os_crypt.encrypted_key;
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .context("encrypted_key is not valid base64")?;
    if key_bytes.len() < DPAPI_PREFIX.len() || &key_bytes[..DPAPI_PREFIX.len()] != DPAPI_PREFIX {
        return Err(anyhow!(
            "encrypted_key missing DPAPI prefix (got {} bytes)",
            key_bytes.len()
        ));
    }
    let dpapi_blob = &key_bytes[DPAPI_PREFIX.len()..];
    let plain = dpapi_decrypt(dpapi_blob).context("DPAPI decrypt of master key failed")?;
    if plain.len() != 32 {
        return Err(anyhow!(
            "decrypted master key has unexpected length {} (want 32)",
            plain.len()
        ));
    }
    Ok(plain)
}

/// Decrypt a Chromium-format AES-GCM blob: `v10`/`v11` prefix + 12-byte
/// nonce + ciphertext+tag. Returns plaintext bytes.
fn aes_gcm_decrypt(cipher: &Aes256Gcm, blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 3 + GCM_NONCE_LEN + 16 {
        return Err(anyhow!("blob too short ({} bytes)", blob.len()));
    }
    let prefix = &blob[..3];
    if prefix != b"v10" && prefix != b"v11" {
        return Err(anyhow!(
            "unknown blob version prefix {:?}",
            std::str::from_utf8(prefix).unwrap_or("?")
        ));
    }
    let nonce = Nonce::from_slice(&blob[3..3 + GCM_NONCE_LEN]);
    let ciphertext = &blob[3 + GCM_NONCE_LEN..];
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("aes-gcm: {}", e))
}

fn discord_app_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let mut p = PathBuf::from(appdata);
    p.push("Discord");
    Some(p)
}

/// Walk the LevelDB directory and collect every `dQw4w9WgXcQ:<base64>`
/// occurrence's base64-decoded ciphertext.
fn scan_for_token_blobs(dir: &Path) -> Result<Vec<Vec<u8>>> {
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e, "ldb" | "log" | "sst"))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                debug!(path = %path.display(), error = %e, "skip unreadable leveldb file");
                continue;
            }
        };
        for hit in scan_bytes_for_prefix(&bytes, TOKEN_PREFIX) {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&hit) {
                if !decoded.is_empty() {
                    blobs.push(decoded);
                }
            }
        }
    }
    // De-duplicate identical encrypted blobs (LevelDB often holds many copies).
    blobs.sort();
    blobs.dedup();
    Ok(blobs)
}

/// For each occurrence of `prefix` in `haystack`, collect the run of base64
/// characters that immediately follows (until first non-base64 byte).
fn scan_bytes_for_prefix<'a>(haystack: &'a [u8], prefix: &'a [u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if prefix.is_empty() || haystack.len() < prefix.len() {
        return out;
    }
    let mut i = 0usize;
    while i + prefix.len() <= haystack.len() {
        if &haystack[i..i + prefix.len()] == prefix {
            let start = i + prefix.len();
            let mut end = start;
            while end < haystack.len() && BASE64_CHARS.contains(&haystack[end]) {
                end += 1;
            }
            // Discord token ciphertexts are typically 200+ bytes after base64
            // encoding; require a sane minimum to avoid spurious matches.
            if end - start >= 80 {
                out.push(haystack[start..end].to_vec());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Call Win32 `CryptUnprotectData` on a single blob.
fn dpapi_decrypt(input: &[u8]) -> Result<Vec<u8>> {
    if input.len() > MAX_DPAPI_INPUT {
        return Err(anyhow!(
            "DPAPI input is {} bytes; refusing to truncate to u32",
            input.len()
        ));
    }
    let cb_data = u32::try_from(input.len()).expect("bounded by MAX_DPAPI_INPUT above");
    let data_in = CRYPT_INTEGER_BLOB {
        cbData: cb_data,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut data_out = CRYPT_INTEGER_BLOB::default();

    // SAFETY: We pass `data_in` by reference; receive `data_out` whose
    // `pbData` we must `LocalFree`. We copy the bytes out before freeing.
    unsafe {
        CryptUnprotectData(
            &data_in,
            Some(ptr::null_mut()),
            Some(ptr::null_mut()),
            Some(ptr::null_mut()),
            Some(ptr::null_mut()),
            0,
            &mut data_out,
        )
        .map_err(|e| anyhow!("CryptUnprotectData failed: {:?}", e))?;
    }

    let len = usize::try_from(data_out.cbData)
        .context("DPAPI output length overflows usize")?;
    let mut buf: Vec<u8> = Vec::with_capacity(len);
    if !data_out.pbData.is_null() {
        if len > 0 {
            // SAFETY: Win32 wrote `len` bytes into `pbData`; capacity == len.
            unsafe {
                ptr::copy_nonoverlapping(data_out.pbData, buf.as_mut_ptr(), len);
                buf.set_len(len);
            }
        }
        // SAFETY: Always free the OS allocation, even on degenerate len=0
        // success paths, to avoid leaks. `LocalFree` returns the unfreed
        // handle (NULL on success); we have no use for it and ignoring is
        // standard for this Win32 API.
        unsafe {
            let _ = LocalFree(HLOCAL(data_out.pbData as _));
        }
    }
    Ok(buf)
}

/// Sanctioned raw-reqwest exception: validate a candidate token directly,
/// outside the `Api`/`ReadOnlyHttp` layer (which itself requires a token).
async fn validate_token(token: &str) -> Result<MeMin> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .connect_timeout(VALIDATE_CONNECT_TIMEOUT)
        .timeout(VALIDATE_TIMEOUT)
        .build()?;
    let resp = client
        .get("https://discord.com/api/v10/users/@me")
        .header(reqwest::header::AUTHORIZATION, token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {}", resp.status().as_u16()));
    }
    let me: MeMin = resp.json().await?;
    Ok(me)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::looks_like_discord_token;
    use base64::Engine;

    #[test]
    fn scan_finds_prefix_and_extracts_base64() {
        let mut buf: Vec<u8> = b"junkjunkjunk".to_vec();
        buf.extend_from_slice(TOKEN_PREFIX);
        // 96-char base64 payload (length is a multiple of 4 → no padding needed).
        let payload =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef";
        assert_eq!(payload.len(), 96);
        assert!(payload.len() >= 80);
        assert_eq!(payload.len() % 4, 0);
        buf.extend_from_slice(payload.as_bytes());
        buf.extend_from_slice(b"!!! garbage after");

        let hits = scan_bytes_for_prefix(&buf, TOKEN_PREFIX);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], payload.as_bytes());
        // ensure base64 decoder accepts the captured run
        assert!(base64::engine::general_purpose::STANDARD
            .decode(&hits[0])
            .is_ok());
    }

    #[test]
    fn scan_ignores_short_runs() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(TOKEN_PREFIX);
        buf.extend_from_slice(b"shortrun");
        let hits = scan_bytes_for_prefix(&buf, TOKEN_PREFIX);
        assert!(hits.is_empty(), "should reject runs under 80 chars");
    }

    #[test]
    fn token_shape_rejects_garbage() {
        assert!(!looks_like_discord_token(""));
        assert!(!looks_like_discord_token("notatoken"));
        assert!(!looks_like_discord_token("only.two"));
        assert!(!looks_like_discord_token("aa.bb.cc!")); // invalid char
        assert!(!looks_like_discord_token(&"a".repeat(300))); // too long
    }

    #[test]
    fn token_shape_accepts_realistic_token() {
        // Three dot-separated base64-url-ish parts of plausible lengths.
        let t = format!(
            "{}.{}.{}",
            "MTAxMjM0NTY3ODkwMTIzNDU2Nw", "GxYz12", "abcdef-ABCDEFG_ijklmnopqrstuvw"
        );
        assert!(looks_like_discord_token(&t));
    }
}
