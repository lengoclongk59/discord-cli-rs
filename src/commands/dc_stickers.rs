//! `discord dc stickers <GUILD>` — list custom stickers.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let stickers = api.get_guild_stickers(&guild_id).await?;

    if stickers.is_empty() {
        output::dim("No custom stickers.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&stickers);
    } else {
        let fmt_type = |t: u32| match t {
            1 => "PNG",
            2 => "APNG",
            3 => "Lottie",
            4 => "GIF",
            _ => "?",
        };
        let rows: Vec<Vec<String>> = stickers
            .iter()
            .map(|s| {
                vec![
                    s.id.clone(),
                    s.name.clone(),
                    s.description.clone().unwrap_or_default(),
                    fmt_type(s.format_type).to_string(),
                ]
            })
            .collect();
        output::print_table(&["id", "name", "description", "format"], &rows);
        output::dim(&format!("\n{} stickers", stickers.len()));
    }
    Ok(())
}
