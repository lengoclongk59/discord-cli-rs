//! `discord search <KEYWORD> [-c CH] [-n N]` — search local archive.

use anyhow::Result;

use crate::cli::SearchArgs;
use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, args: SearchArgs) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;
    let chan = resolve::resolve_channel(&db, args.channel.as_deref())?;

    let rows = db.search(&args.keyword, chan.as_deref(), args.limit)?;
    if ctx.json {
        output::print_json(&rows);
        return Ok(());
    }
    if rows.is_empty() {
        output::dim("No matches.");
        return Ok(());
    }
    for m in &rows {
        println!("{}", output::format_message(m));
    }
    output::dim(&format!("\n{} matches", rows.len()));
    Ok(())
}
