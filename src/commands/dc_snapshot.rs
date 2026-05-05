//! `discord dc snapshot <GUILD>` — export full guild state as JSON.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;
use crate::types::ServerSnapshot;

pub async fn run(ctx: &Ctx, guild: &str, output_file: Option<PathBuf>) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;

    output::dim("Fetching guild info...");
    let info = api.get_guild_info(&guild_id).await?;
    let guild_name = info.name.clone();

    let mut failures: Vec<String> = Vec::new();
    macro_rules! fetch_or_warn {
        ($label:expr, $fut:expr, $default:expr) => {{
            output::dim(&format!("Fetching {}...", $label));
            match $fut.await {
                Ok(v) => v,
                Err(e) => {
                    output::warn(&format!("{}: {}", $label, e));
                    failures.push($label.to_string());
                    $default
                }
            }
        }};
    }

    let channels = fetch_or_warn!("channels", api.list_text_channels(&guild_id), Vec::new());
    let roles = fetch_or_warn!("roles", api.get_guild_roles(&guild_id), Vec::new());
    let emojis = fetch_or_warn!("emojis", api.get_guild_emojis(&guild_id), Vec::new());
    let stickers = fetch_or_warn!("stickers", api.get_guild_stickers(&guild_id), Vec::new());
    let members = fetch_or_warn!(
        "members",
        api.list_guild_members(&guild_id, 1000),
        Vec::new()
    );
    let threads = match api.get_active_threads(&guild_id).await {
        Ok(r) => r.threads,
        Err(e) => {
            output::warn(&format!("threads: {}", e));
            failures.push("threads".to_string());
            Vec::new()
        }
    };

    let snapshot = ServerSnapshot {
        guild: info,
        channels,
        roles,
        emojis,
        stickers,
        members,
        threads,
    };

    let json = serde_json::to_string_pretty(&snapshot)?;

    if let Some(path) = output_file {
        let mut f = fs::File::create(&path)?;
        f.write_all(json.as_bytes())?;
        output::success(&format!(
            "Snapshot of '{}' saved to {} ({} channels, {} roles, {} members)",
            guild_name,
            path.display(),
            snapshot.channels.len(),
            snapshot.roles.len(),
            snapshot.members.len(),
        ));
    } else {
        println!("{}", json);
    }
    if !failures.is_empty() {
        // Non-zero exit so scripts can detect a degraded snapshot.
        return Err(anyhow::anyhow!(
            "snapshot completed with failures: {}",
            failures.join(", ")
        ));
    }
    Ok(())
}
