//! `discord recent [-c CH] [--hours H] [-n N]` — newest stored messages.

use anyhow::Result;

use crate::cli::RecentArgs;
use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, args: RecentArgs) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let chan = resolve::resolve_channel(&db, args.channel.as_deref())?;

    let rows = db.recent(chan.as_deref(), args.hours, args.limit)?;
    if ctx.json {
        output::print_json(&rows);
        return Ok(());
    }
    if rows.is_empty() {
        output::dim("No messages.");
        return Ok(());
    }
    for m in &rows {
        println!("{}", output::format_message(m));
    }
    Ok(())
}
