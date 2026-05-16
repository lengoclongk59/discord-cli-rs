//! `discord dc history <CHANNEL> [-n N]` — fetch historical messages backward.
//!
//! Uses an `oldest_msg_id` cursor stored in `meta.history_cursor` so an
//! interrupted backfill can resume from where it left off instead of
//! re-fetching from the newest message every time.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::db::Db;
use crate::output;
use crate::types::ChannelContext;

pub async fn run(ctx: &Ctx, channel: &str, limit: u32) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);
    let mut db = Db::open(&ctx.db_path)?;

    let meta = match api.resolve_channel_context(channel).await {
        Ok(m) => m,
        Err(e) => {
            output::warn(&format!(
                "could not resolve metadata for channel {} ({}); rows will lack guild/channel names",
                channel, e
            ));
            ChannelContext::default()
        }
    };

    // Resume from the oldest message we already have (if any), so a
    // re-run continues backward instead of re-walking the most recent N.
    let resume_before = db.history_cursor(channel)?;
    if let Some(id) = &resume_before {
        output::dim(&format!(
            "Resuming history of {} from msg_id < {}",
            channel, id
        ));
    } else {
        output::dim(&format!(
            "Fetching up to {} messages from channel {}",
            limit, channel
        ));
    }

    let page = api
        .fetch_messages_page(channel, None, resume_before.as_deref(), limit, &meta)
        .await?;
    let total = page.messages.len();
    let inserted = db.insert_batch(&page.messages)?;

    let oldest_str = page.oldest_msg_id.as_ref().map(|m| m.to_string());
    if let Some(oldest) = &oldest_str {
        db.set_history_cursor(channel, oldest)?;
    }

    if ctx.json {
        output::print_json(&serde_json::json!({
            "fetched": total,
            "stored": inserted,
            "channel_id": channel,
            "hit_limit": page.hit_limit,
            "oldest_msg_id": oldest_str,
        }));
    } else {
        output::success(&format!("Fetched {}, stored {} new", total, inserted));
        if page.hit_limit {
            output::warn(&format!(
                "hit --limit ({}); re-run `dc history` to continue backward",
                limit
            ));
        }
    }
    Ok(())
}
