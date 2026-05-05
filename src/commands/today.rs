//! `discord today` — messages since local midnight.

use anyhow::Result;

use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, channel: Option<&str>) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let channel_id = resolve::resolve_channel(&db, channel)?;

    let msgs = db.today(channel_id.as_deref())?;

    if ctx.json {
        output::print_json(&msgs);
        return Ok(());
    }

    if msgs.is_empty() {
        output::dim("No messages today.");
        return Ok(());
    }

    let mut current_channel = String::new();
    for m in &msgs {
        let chan = m.channel_name.as_deref().unwrap_or(&m.channel_id);
        if chan != current_channel {
            if !current_channel.is_empty() {
                println!();
            }
            println!("--- #{} ---", chan);
            current_channel = chan.to_string();
        }
        let time = m.timestamp.format("%H:%M:%S").to_string();
        println!("  {} {}: {}", time, m.sender_name, m.content.replace('\n', " "));
    }
    output::dim(&format!("\n{} messages today", msgs.len()));
    Ok(())
}
