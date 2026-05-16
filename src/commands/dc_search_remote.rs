//! `discord dc search <GUILD> <KEYWORD>` — remote Discord search.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(
    ctx: &Ctx,
    guild: &str,
    keyword: &str,
    channel: Option<&str>,
    limit: u32,
) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let results = api
        .search_guild_messages(&guild_id, keyword, channel, limit)
        .await?;

    if ctx.json {
        output::print_json(&results);
    } else if results.is_empty() {
        output::dim("No results found.");
    } else {
        for m in &results {
            println!("{}", output::format_message(m));
        }
        output::dim(&format!("\n{} results", results.len()));
    }
    Ok(())
}
