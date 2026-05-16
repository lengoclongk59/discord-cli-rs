//! `discord dc roles <GUILD>` — list guild roles.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let mut roles = api.get_guild_roles(&guild_id).await?;
    roles.sort_by_key(|r| std::cmp::Reverse(r.position));

    if roles.is_empty() {
        output::dim("No roles.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&roles);
    } else {
        let rows: Vec<Vec<String>> = roles
            .iter()
            .map(|r| {
                let color = if r.color > 0 {
                    format!("#{:06X}", r.color)
                } else {
                    String::new()
                };
                vec![
                    r.id.clone(),
                    r.name.clone(),
                    color,
                    r.position.to_string(),
                    if r.hoist { "yes" } else { "" }.to_string(),
                    if r.mentionable { "yes" } else { "" }.to_string(),
                ]
            })
            .collect();
        output::print_table(&["id", "name", "color", "pos", "hoist", "mention"], &rows);
        output::dim(&format!("\n{} roles", roles.len()));
    }
    Ok(())
}
