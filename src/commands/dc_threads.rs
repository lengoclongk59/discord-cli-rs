//! `discord dc threads <GUILD>` — list active threads.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;
use crate::wire_enums::ThreadState;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
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
                let state = t
                    .thread_metadata
                    .as_ref()
                    .map(|m| ThreadState::from_flags(m.archived, m.locked).to_string())
                    .unwrap_or_else(|| "-".to_string());
                vec![
                    t.id.clone(),
                    t.name.clone().unwrap_or_default(),
                    t.parent_id.clone().unwrap_or_default(),
                    t.message_count.map(|c| c.to_string()).unwrap_or_default(),
                    t.member_count.map(|c| c.to_string()).unwrap_or_default(),
                    state,
                ]
            })
            .collect();
        output::print_table(
            &["id", "name", "parent", "messages", "members", "state"],
            &rows,
        );
        output::dim(&format!("\n{} active threads", resp.threads.len()));
    }
    Ok(())
}
