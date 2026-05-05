//! `discord stats` — per-channel message counts.

use anyhow::Result;

use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let rows = db.stats()?;
    if ctx.json {
        let data: Vec<serde_json::Value> = rows
            .iter()
            .map(|(cid, name, n)| {
                serde_json::json!({
                    "channel_id": cid,
                    "channel_name": name,
                    "count": n,
                })
            })
            .collect();
        output::print_json(&data);
        return Ok(());
    }
    let display: Vec<Vec<String>> = rows
        .iter()
        .map(|(cid, name, n)| vec![name.clone().unwrap_or_default(), cid.clone(), n.to_string()])
        .collect();
    output::print_table(&["channel", "id", "count"], &display);
    Ok(())
}
