//! `discord dc download <CHANNEL>` — download attachments from stored messages.
//!
//! Source of truth is the `attachments` table populated during sync, not
//! a substring scan of `content`. The latter is kept as a fallback for
//! channels that were synced before the v2 schema migration.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::fs;

use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

const MAX_FILENAME_LEN: usize = 200;
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Windows reserved device names — cannot be used as filenames even with
/// extensions. Matched case-insensitively against the file *stem*.
const WIN_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub async fn run(ctx: &Ctx, channel: &str, output_dir: &Path, hours: Option<i64>) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let channel_id = resolve::resolve_channel_required(&db, channel)?;

    fs::create_dir_all(output_dir).await?;
    // Canonicalize once so the per-file containment check is meaningful
    // even when output_dir contains `..` segments.
    let output_dir_canonical = fs::canonicalize(output_dir)
        .await
        .map_err(|e| anyhow!("output dir {}: {}", output_dir.display(), e))?;

    // Primary source: persisted attachment metadata.
    let mut entries: Vec<(String, String)> = db
        .attachments_for_channel(&channel_id, hours)?
        .into_iter()
        .map(|(_msg, _attach, filename, url, _ct, _size)| (url, filename))
        .collect();

    // Legacy fallback: scan stored content for embedded URLs (covers
    // archives synced before the attachments table existed).
    if entries.is_empty() {
        let msgs = db.recent(Some(&channel_id), hours, 100_000)?;
        for m in &msgs {
            for word in m.content.split_whitespace() {
                if word.starts_with("https://cdn.discordapp.com/attachments/")
                    || word.starts_with("https://media.discordapp.net/attachments/")
                {
                    let clean = word.split('?').next().unwrap_or(word).to_string();
                    let raw = clean.rsplit('/').next().unwrap_or("file").to_string();
                    entries.push((clean, raw));
                }
            }
        }
    }

    if entries.is_empty() {
        output::dim("No downloadable attachments found.");
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("building HTTP client for attachment downloads")?;
    let mut downloaded = 0usize;
    let mut errors = 0usize;
    let mut name_counter: HashMap<String, usize> = HashMap::with_capacity(entries.len());

    for (url, raw_name) in &entries {
        let base = match safe_filename(raw_name) {
            Some(s) => s,
            None => {
                output::err(&format!("Skipping unsafe filename: {:?}", raw_name));
                errors += 1;
                continue;
            }
        };

        let count_entry = name_counter.entry(base.clone()).or_insert(0);
        let n = *count_entry;
        let filename = if n == 0 {
            base.clone()
        } else {
            let stem = Path::new(&base)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let ext = Path::new(&base)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ext.is_empty() {
                format!("{}_{}", stem, n)
            } else {
                format!("{}_{}.{}", stem, n, ext)
            }
        };
        *count_entry += 1;

        let dest = output_dir_canonical.join(&filename);
        if Path::new(&filename)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            output::err(&format!("Path traversal blocked: {:?}", filename));
            errors += 1;
            continue;
        }
        if dest.exists() {
            continue;
        }

        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Some(len) = resp.content_length() {
                    if len > MAX_FILE_BYTES {
                        output::err(&format!(
                            "Skipping {} ({}MB, too large)",
                            filename,
                            len / 1_048_576
                        ));
                        errors += 1;
                        continue;
                    }
                }
                match resp.bytes().await {
                    Ok(bytes) => {
                        if let Err(e) = fs::write(&dest, &bytes).await {
                            output::err(&format!("Write failed {}: {}", filename, e));
                            errors += 1;
                        } else {
                            downloaded += 1;
                        }
                    }
                    Err(e) => {
                        output::err(&format!("Download failed {}: {}", filename, e));
                        errors += 1;
                    }
                }
            }
            Ok(resp) => {
                output::err(&format!("HTTP {} for {}", resp.status(), filename));
                errors += 1;
            }
            Err(e) => {
                output::err(&format!("Request failed {}: {}", filename, e));
                errors += 1;
            }
        }
    }

    if ctx.json {
        output::print_json(&serde_json::json!({
            "found": entries.len(),
            "downloaded": downloaded,
            "errors": errors,
            "output_dir": output_dir_canonical.display().to_string(),
        }));
    } else {
        output::success(&format!(
            "Downloaded {} files to {} ({} errors)",
            downloaded,
            output_dir_canonical.display(),
            errors,
        ));
    }
    Ok(())
}

/// Reduce a possibly-arbitrary string to a safe local filename or reject.
/// Strips path components, NUL bytes, trailing dots/spaces (Windows), caps
/// length, and rejects Windows reserved device names.
fn safe_filename(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    // file_name() drops directory traversal and absolute components.
    let raw: PathBuf = Path::new(input).file_name()?.into();
    let s = raw.to_str()?.to_string();
    if s.contains('\0') || s.is_empty() {
        return None;
    }
    let mut s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        return None;
    }
    if s.len() > MAX_FILENAME_LEN {
        // Preserve extension when we have to truncate.
        let ext = Path::new(&s)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let stem = Path::new(&s)
            .file_stem()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let keep = MAX_FILENAME_LEN.saturating_sub(ext.len() + 1);
        s = if ext.is_empty() {
            stem.chars().take(MAX_FILENAME_LEN).collect()
        } else {
            format!("{}.{}", stem.chars().take(keep).collect::<String>(), ext)
        };
    }
    let stem = Path::new(&s)
        .file_stem()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if WIN_RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        // Prefix to disambiguate. "nul.png" → "_nul.png".
        return Some(format!("_{}", s));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        assert_eq!(safe_filename("../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(safe_filename("..\\evil.exe").as_deref(), Some("evil.exe"));
    }

    #[test]
    fn handles_reserved_names() {
        assert_eq!(safe_filename("nul.png").as_deref(), Some("_nul.png"));
        assert_eq!(safe_filename("CON").as_deref(), Some("_CON"));
        assert_eq!(safe_filename("LPT1.txt").as_deref(), Some("_LPT1.txt"));
    }

    #[test]
    fn strips_trailing_dot_and_space() {
        assert_eq!(safe_filename("file...").as_deref(), Some("file"));
        assert_eq!(safe_filename("file  ").as_deref(), Some("file"));
    }

    #[test]
    fn caps_length() {
        let long = "a".repeat(400) + ".png";
        let s = safe_filename(&long).unwrap();
        assert!(s.len() <= MAX_FILENAME_LEN);
        assert!(s.ends_with(".png"));
    }

    #[test]
    fn rejects_empty_or_nul() {
        assert!(safe_filename("").is_none());
        assert!(safe_filename("\0").is_none());
    }
}
