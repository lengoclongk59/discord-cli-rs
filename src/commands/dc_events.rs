//! `discord dc events <GUILD>` — list scheduled events.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let events = api.get_scheduled_events(&guild_id).await?;

    if events.is_empty() {
        output::dim("No scheduled events.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&events);
    } else {
        let status_name = |s: u32| match s {
            1 => "Scheduled",
            2 => "Active",
            3 => "Completed",
            4 => "Cancelled",
            _ => "?",
        };
        let rows: Vec<Vec<String>> = events
            .iter()
            .map(|e| {
                vec![
                    e.id.clone(),
                    e.name.clone(),
                    status_name(e.status).to_string(),
                    e.scheduled_start_time.as_deref().unwrap_or("").chars().take(16).collect(),
                    e.user_count.map(|c| c.to_string()).unwrap_or_default(),
                ]
            })
            .collect();
        output::print_table(&["id", "name", "status", "start", "interested"], &rows);
        output::dim(&format!("\n{} events", events.len()));
    }
    Ok(())
}
