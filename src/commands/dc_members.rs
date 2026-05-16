//! `discord dc members <GUILD>` — list guild members.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str, limit: u32) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let members = api.list_guild_members(&guild_id, limit).await?;

    if members.is_empty() {
        output::dim("No members returned (may require Privileged Gateway Intents).");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&members);
    } else {
        let rows: Vec<Vec<String>> = members
            .iter()
            .map(|m| {
                let user = m.user.as_ref();
                vec![
                    user.map(|u| u.id.clone()).unwrap_or_default(),
                    user.map(|u| u.username.clone()).unwrap_or_default(),
                    user.and_then(|u| u.global_name.clone()).unwrap_or_default(),
                    m.nick.clone().unwrap_or_default(),
                    format!("{}", m.roles.len()),
                ]
            })
            .collect();
        output::print_table(
            &["id", "username", "display name", "nickname", "roles"],
            &rows,
        );
        output::dim(&format!("\n{} members", members.len()));
    }
    Ok(())
}
