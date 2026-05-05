//! `discord dc sync <CHANNEL_ID> [-n N]` — incremental sync of one channel.
//!
//! Resolves the channel's parent guild + names via the API before insert so
//! that name-keyed query commands (`search -c`, `recent -c`) work even when
//! `dc sync-all` was never run.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::db::Db;
use crate::output;
use crate::types::ChannelContext;

pub async fn run(ctx: &Ctx, channel: &str, limit: u32) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);
    let mut db = Db::open(&ctx.db_path)?;

    // Resolve channel + parent guild metadata so rows include names.
    // Failure here used to be silently swallowed via `unwrap_or_default`,
    // which produced rows with empty guild/channel names that confused
    // every subsequent name-keyed query. Now we warn and continue with a
    // bare context — the caller still gets messages, just without names.
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

    let last = db.last_msg_id(channel)?;
    if let Some(id) = &last {
        output::dim(&format!("Syncing channel {} from msg_id > {}", channel, id));
    } else {
        output::dim(&format!("Initial sync of channel {}", channel));
    }

    let page = api
        .fetch_messages_page(channel, last.as_deref(), None, limit, &meta)
        .await?;
    let total = page.messages.len();
    let inserted = db.insert_batch(&page.messages)?;
    output::success(&format!("Fetched {}, stored {} new", total, inserted));
    if page.hit_limit {
        output::warn(&format!(
            "hit --limit ({}); more new messages may exist — re-run sync to continue",
            limit
        ));
    }
    Ok(())
}
