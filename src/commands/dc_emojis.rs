//! `discord dc emojis <GUILD>` — list custom emojis.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let emojis = api.get_guild_emojis(&guild_id).await?;

    if emojis.is_empty() {
        output::dim("No custom emojis.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&emojis);
    } else {
        let rows: Vec<Vec<String>> = emojis
            .iter()
            .map(|e| {
                vec![
                    e.id.clone().unwrap_or_default(),
                    e.name.clone().unwrap_or_default(),
                    if e.animated { "yes" } else { "" }.to_string(),
                    if e.available { "yes" } else { "" }.to_string(),
                    if e.managed { "yes" } else { "" }.to_string(),
                ]
            })
            .collect();
        output::print_table(&["id", "name", "animated", "available", "managed"], &rows);
        output::dim(&format!("\n{} emojis", emojis.len()));
    }
    Ok(())
}
