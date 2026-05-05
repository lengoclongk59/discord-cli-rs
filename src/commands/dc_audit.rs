//! `discord dc audit <GUILD>` — show recent audit log entries.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx, guild: &str, limit: u8) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    let guild_id = api.resolve_guild_id(guild).await?;
    let resp = api.get_guild_audit_logs(&guild_id, limit).await?;

    if resp.audit_log_entries.is_empty() {
        output::dim("No audit log entries.");
        return Ok(());
    }

    if ctx.json {
        output::print_json(&resp.audit_log_entries);
    } else {
        let action_name = |t: u32| match t {
            1 => "Guild Update",
            10..=12 => "Channel Op",
            13..=15 => "Overwrite Op",
            20..=28 => "Member Op",
            30..=32 => "Role Op",
            40..=42 => "Invite Op",
            50..=52 => "Webhook Op",
            60..=62 => "Emoji Op",
            72..=75 => "Message Op",
            80..=84 => "Integration Op",
            90..=92 => "Sticker Op",
            100..=102 => "Event Op",
            110..=112 => "Thread Op",
            140..=145 => "AutoMod Op",
            _ => "Other",
        };
        let rows: Vec<Vec<String>> = resp
            .audit_log_entries
            .iter()
            .map(|e| {
                vec![
                    e.id.clone(),
                    e.user_id.clone().unwrap_or_default(),
                    action_name(e.action_type).to_string(),
                    e.target_id.clone().unwrap_or_default(),
                    e.reason.clone().unwrap_or_default(),
                ]
            })
            .collect();
        output::print_table(&["id", "user", "action", "target", "reason"], &rows);
        output::dim(&format!("\n{} entries", resp.audit_log_entries.len()));
    }
    Ok(())
}
