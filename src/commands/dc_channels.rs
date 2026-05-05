//! `discord dc channels <GUILD_ID_OR_NAME>` — list text channels.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;

    let channels = api.list_text_channels(&guild_id).await?;
    if ctx.json {
        output::print_json(&channels);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = channels
        .iter()
        .map(|c| {
            vec![
                c.id.clone(),
                format!("#{}", c.name.as_deref().unwrap_or("")),
                {
                    let topic = c.topic.clone().unwrap_or_default();
                    if topic.chars().count() > 50 {
                        let truncated: String = topic.chars().take(50).collect();
                        format!("{}…", truncated)
                    } else {
                        topic
                    }
                },
            ]
        })
        .collect();
    output::print_table(&["id", "name", "topic"], &rows);
    output::dim(&format!("\nTotal: {} text channels", channels.len()));
    Ok(())
}
