//! `discord dc threads <GUILD>` — list active threads.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let resp = api.get_active_threads(&guild_id).await?;

    if resp.threads.is_empty() {
        output::dim("No active threads.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&resp.threads);
    } else {
        let rows: Vec<Vec<String>> = resp
            .threads
            .iter()
            .map(|t| {
                let archived = t
                    .thread_metadata
                    .as_ref()
                    .map(|m| if m.archived { "yes" } else { "no" })
                    .unwrap_or("-");
                vec![
                    t.id.clone(),
                    t.name.clone().unwrap_or_default(),
                    t.parent_id.clone().unwrap_or_default(),
                    t.message_count.map(|c| c.to_string()).unwrap_or_default(),
                    t.member_count.map(|c| c.to_string()).unwrap_or_default(),
                    archived.to_string(),
                ]
            })
            .collect();
        output::print_table(&["id", "name", "parent", "messages", "members", "archived"], &rows);
        output::dim(&format!("\n{} active threads", resp.threads.len()));
    }
    Ok(())
}
