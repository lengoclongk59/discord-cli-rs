//! Token resolution chain and `.env` upsert.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use tempfile::NamedTempFile;
use zeroize::Zeroize;

/// Resolve the user token using this priority:
/// 1. `cli_flag` (e.g. `--token <T>`)
/// 2. `DISCORD_TOKEN` env var
/// 3. `./.env` -> `DISCORD_TOKEN=...`
pub fn resolve_token(cli_flag: Option<&str>) -> Result<String> {
    if let Some(t) = cli_flag.filter(|s| !s.is_empty()) {
        return Ok(t.to_string());
    }
    if let Ok(t) = std::env::var("DISCORD_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let _ = dotenvy::dotenv();
    if let Ok(t) = std::env::var("DISCORD_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    Err(anyhow!(
        "No Discord token found. Run `discord auth --save`, set $DISCORD_TOKEN, or pass --token <T>."
    ))
}

/// Upsert `DISCORD_TOKEN=<token>` into `./.env`. Creates the file if
/// missing. Preserves all other lines.
///
/// Write semantics: write to a tempfile in the same directory, fsync, then
/// rename. Pre-rename crashes leave the original .env intact. After write,
/// best-effort 0600 on POSIX so the token isn't world-readable.
pub fn write_token_to_env(token: &str) -> Result<PathBuf> {
    let path = Path::new(".env").to_path_buf();
    let existing = fs::read_to_string(&path).unwrap_or_default();

    let mut found = false;
    let mut new_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("DISCORD_TOKEN") {
            // Match either `DISCORD_TOKEN=...` or `DISCORD_TOKEN =...`
            let rest = rest.trim_start();
            if rest.starts_with('=') {
                new_lines.push(format!("DISCORD_TOKEN={}", token));
                found = true;
                continue;
            }
        }
        new_lines.push(line.to_string());
    }
    if !found {
        new_lines.push(format!("DISCORD_TOKEN={}", token));
    }
    let mut out = new_lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }

    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let mut tmp = if let Some(p) = parent {
        NamedTempFile::new_in(p)
    } else {
        NamedTempFile::new_in(".")
    }
    .context("creating .env tempfile")?;
    tmp.write_all(out.as_bytes())?;
    tmp.as_file().sync_all().ok();

    // Tighten perms before the rename so the file is never visible to
    // other users (relevant on POSIX; on Windows ACLs inherit from parent).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600));
    }

    tmp.persist(&path)
        .map_err(|e| anyhow!("persisting .env: {}", e))?;

    // Best-effort scrub of the in-memory copy.
    out.zeroize();
    Ok(path)
}

/// Lightweight shape check shared across platform-specific token finders.
/// Discord tokens look like `<base64-ish>.<base64-ish>.<base64-ish>` with
/// total length 40–200 chars.
pub fn looks_like_discord_token(s: &str) -> bool {
    let len = s.len();
    if !(40..=200).contains(&len) {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// Mask the token to `<first4>…<last4>` (or `<masked>` if shorter than 8 chars).
pub fn mask_token(t: &str) -> String {
    if t.len() < 8 {
        return "<masked>".to_string();
    }
    let head: String = t.chars().take(4).collect();
    let tail_skip = t.chars().count().saturating_sub(4);
    let tail: String = t.chars().skip(tail_skip).collect();
    format!("{}…{}", head, tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short_token() {
        assert_eq!(mask_token(""), "<masked>");
        assert_eq!(mask_token("abc"), "<masked>");
        assert_eq!(mask_token("1234567"), "<masked>");
    }

    #[test]
    fn mask_normal_token() {
        let t = "abcdefghijklmnopqrstuvwxyz";
        assert_eq!(mask_token(t), "abcd…wxyz");
    }

    #[test]
    fn mask_typical_discord_token_only_reveals_first4_last4() {
        let t = "FAKE_TOKEN_aaaa.bbbbbb.cccccccccccccccccccccccccccc";
        let masked = mask_token(t);
        assert!(masked.starts_with("FAKE"));
        assert!(masked.ends_with("cccc"));
        // Total chars revealed should be exactly 8 (4 head + 4 tail).
        let revealed = masked.replace('…', "");
        assert_eq!(revealed.chars().count(), 8);
    }

    #[test]
    fn resolve_token_from_flag() {
        let got = resolve_token(Some("from-flag")).unwrap();
        assert_eq!(got, "from-flag");
    }
}
