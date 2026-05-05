//! `discord timeline` — activity histogram.

use anyhow::Result;

use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, channel: Option<&str>, hours: Option<i64>, by: &str) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let channel_id = resolve::resolve_channel(&db, channel)?;

    let rows = db.timeline(channel_id.as_deref(), hours, by)?;

    if ctx.json {
        let data: Vec<serde_json::Value> = rows
            .iter()
            .map(|(period, count)| {
                serde_json::json!({ "period": period, "count": count })
            })
            .collect();
        output::print_json(&data);
        return Ok(());
    }

    if rows.is_empty() {
        output::dim("No timeline data.");
        return Ok(());
    }

    let max_count = rows.iter().map(|(_, c)| *c).max().unwrap_or(1);
    let bar_width = 40usize;

    for (period, count) in &rows {
        let width = if max_count > 0 {
            (*count as usize * bar_width) / max_count as usize
        } else {
            0
        };
        let bar: String = "\u{2588}".repeat(width);
        println!("{}  {:>6}  {}", period, count, bar);
    }
    Ok(())
}
