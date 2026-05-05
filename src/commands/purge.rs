//! `discord purge <CHANNEL>` — delete stored messages for a channel.

use std::io::{self, Write};

use anyhow::Result;

use crate::cli::PurgeArgs;
use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, args: PurgeArgs) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;

    let channel_id = resolve::resolve_channel_required(&db, &args.channel)?;

    let count = db.count_channel(&channel_id)?;
    if count == 0 {
        output::dim(&format!(
            "No messages for channel {} in the local archive.",
            channel_id
        ));
        return Ok(());
    }

    if !args.yes {
        print!(
            "Delete {} messages for channel {}? Type 'yes' to confirm: ",
            count, channel_id
        );
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim() != "yes" {
            output::dim("Aborted.");
            return Ok(());
        }
    }

    // Re-open as mutable for the delete
    let deleted = db.purge(&channel_id)?;

    if ctx.json {
        output::print_json(&serde_json::json!({
            "deleted": deleted,
            "channel_id": channel_id,
        }));
    } else {
        output::success(&format!(
            "Deleted {} messages for channel {}",
            deleted, channel_id
        ));
    }
    Ok(())
}
