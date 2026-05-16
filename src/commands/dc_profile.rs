//! `discord dc profile [USER_ID]` — show user profile.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, user_id: Option<&str>) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    let uid = match user_id {
        Some(id) => id.to_string(),
        None => api.get_me().await?.id,
    };

    let profile = api.get_user_profile(&uid).await?;

    if ctx.json {
        output::print_json(&profile);
        return Ok(());
    }

    let user = profile.user.as_ref();
    let profile_data = profile.user_profile.as_ref();

    let mut rows: Vec<Vec<String>> = vec![
        vec!["ID".into(), user.map(|u| u.id.clone()).unwrap_or_default()],
        vec!["Username".into(), user.map(|u| format!("@{}", u.username)).unwrap_or_default()],
        vec!["Display Name".into(), user.and_then(|u| u.global_name.clone()).unwrap_or_default()],
        vec!["Bio".into(), profile_data.and_then(|p| p.bio.clone()).unwrap_or_default()],
        vec!["Pronouns".into(), profile_data.and_then(|p| p.pronouns.clone()).unwrap_or_default()],
        vec!["Premium Since".into(), profile.premium_since.clone().unwrap_or_default()],
    ];

    if !profile.connected_accounts.is_empty() {
        let accounts: Vec<String> = profile
            .connected_accounts
            .iter()
            .map(|a| {
                format!(
                    "{}: {}",
                    a.type_.as_deref().unwrap_or("?"),
                    a.name.as_deref().unwrap_or("?")
                )
            })
            .collect();
        rows.push(vec!["Accounts".into(), accounts.join(", ")]);
    }

    if !profile.mutual_guilds.is_empty() {
        rows.push(vec!["Mutual Guilds".into(), profile.mutual_guilds.len().to_string()]);
    }

    output::print_table(&["Field", "Value"], &rows);
    Ok(())
}
