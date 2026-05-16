//! `discord dc info <GUILD>` — show guild details.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let info = api.get_guild_info(&guild_id).await?;

    if ctx.json {
        output::print_json(&info);
    } else {
        let rows: Vec<Vec<String>> = vec![
            vec!["ID".into(), info.id.clone()],
            vec!["Name".into(), info.name.clone()],
            vec![
                "Description".into(),
                info.description.clone().unwrap_or_default(),
            ],
            vec!["Owner ID".into(), info.owner_id.clone().unwrap_or_default()],
            vec![
                "Members".into(),
                info.approximate_member_count
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            ],
            vec![
                "Online".into(),
                info.approximate_presence_count
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            ],
            vec![
                "Locale".into(),
                info.preferred_locale.clone().unwrap_or_default(),
            ],
            vec!["Boost Tier".into(), info.premium_tier.to_string()],
            vec![
                "Boosts".into(),
                info.premium_subscription_count
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
            ],
        ];
        output::print_table(&["Field", "Value"], &rows);
    }
    Ok(())
}
