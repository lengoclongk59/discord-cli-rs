//! `discord dc audit <GUILD>` — show recent audit log entries.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;
use crate::wire_enums::AuditAction;

pub async fn run(ctx: &Ctx, guild: &str, limit: u8) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
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
        let rows: Vec<Vec<String>> = resp
            .audit_log_entries
            .iter()
            .map(|e| {
                vec![
                    e.id.clone(),
                    e.user_id.clone().unwrap_or_default(),
                    AuditAction::from(e.action_type).to_string(),
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
