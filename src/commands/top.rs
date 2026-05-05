//! `discord top` — most active senders.

use anyhow::Result;

use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, channel: Option<&str>, hours: Option<i64>, limit: i64) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let channel_id = resolve::resolve_channel(&db, channel)?;

    let rows = db.top_senders(channel_id.as_deref(), hours, limit)?;

    if ctx.json {
        let data: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, count, first, last)| {
                serde_json::json!({
                    "sender": name,
                    "count": count,
                    "first_message": first,
                    "last_message": last,
                })
            })
            .collect();
        output::print_json(&data);
        return Ok(());
    }

    if rows.is_empty() {
        output::dim("No messages found.");
        return Ok(());
    }

    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .enumerate()
        .map(|(i, (name, count, first, last))| {
            vec![
                format!("{}", i + 1),
                name.clone(),
                count.to_string(),
                first.chars().take(10).collect(),
                last.chars().take(10).collect(),
            ]
        })
        .collect();
    output::print_table(&["#", "sender", "messages", "first seen", "last seen"], &table_rows);
    Ok(())
}
