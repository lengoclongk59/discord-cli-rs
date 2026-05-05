//! `discord dc relationships` — show friends, blocked, pending.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let relationships = api.get_relationships().await?;

    if relationships.is_empty() {
        output::dim("No relationships.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&relationships);
    } else {
        let rows: Vec<Vec<String>> = relationships
            .iter()
            .map(|r| {
                let type_name = match r.type_ {
                    1 => "Friend",
                    2 => "Blocked",
                    3 => "Pending In",
                    4 => "Pending Out",
                    5 => "Implicit",
                    _ => "Unknown",
                };
                let user = r.user.as_ref();
                vec![
                    user.map(|u| u.id.clone()).unwrap_or_default(),
                    user.map(|u| u.username.clone()).unwrap_or_default(),
                    user.and_then(|u| u.global_name.clone()).unwrap_or_default(),
                    type_name.to_string(),
                    r.nickname.clone().unwrap_or_default(),
                ]
            })
            .collect();
        output::print_table(&["id", "username", "display name", "type", "nickname"], &rows);
        output::dim(&format!("\n{} relationships", relationships.len()));
    }
    Ok(())
}
