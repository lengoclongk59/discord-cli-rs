//! `discord dc pins <CHANNEL>` — show pinned messages.

use anyhow::Result;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;
use crate::types::{ChannelContext, StoredMessage};

pub async fn run(ctx: &Ctx, channel: &str) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    // Resolve metadata so the rendered output includes guild/channel name.
    // Failure is non-fatal — fall back to empty context.
    let meta = api
        .resolve_channel_context(channel)
        .await
        .unwrap_or_else(|e| {
            output::warn(&format!("could not resolve channel context: {}", e));
            ChannelContext::default()
        });

    let pins = api.get_pins(channel).await?;

    if pins.is_empty() {
        output::dim("No pinned messages.");
        return Ok(());
    }

    let messages: Vec<StoredMessage> = pins
        .iter()
        .map(|r| StoredMessage::try_from_raw_with_ctx(r, channel, &meta))
        .collect::<Result<Vec<_>>>()?;

    if ctx.json {
        output::print_json(&messages);
    } else {
        for m in &messages {
            println!("{}", output::format_message(m));
        }
        output::dim(&format!("\n{} pinned messages", messages.len()));
    }
    Ok(())
}
