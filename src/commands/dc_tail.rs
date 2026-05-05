//! `discord dc tail <CHANNEL>` — stream messages in real time via the
//! Gateway, persisting MESSAGE_CREATE/UPDATE/DELETE into the local archive.
//!
//! Snapshot policy: the initial REST fetch starts from `last_msg_id` so a
//! restart no longer re-prints (and re-stores) the same `--limit` window.
//! When the DB is empty we fall back to the most recent `--limit`.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::DateTime;
use colored::Colorize;
use tokio::sync::Mutex;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::db::Db;
use crate::output;
use crate::types::ChannelContext;

pub async fn run(ctx: &Ctx, channel: &str, limit: u32, once: bool) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);

    if once {
        let messages = api.fetch_messages(channel, None, limit).await?;
        for m in &messages {
            println!("{}", output::format_message(m));
        }
        return Ok(());
    }

    let meta = api
        .resolve_channel_context(channel)
        .await
        .unwrap_or_else(|e| {
            output::warn(&format!("could not resolve channel context: {}", e));
            ChannelContext::default()
        });

    let db = Arc::new(Mutex::new(Db::open(&ctx.db_path)?));

    output::dim(&format!(
        "Connecting to Gateway for channel {}... press Ctrl+C to stop",
        channel
    ));

    let target_channel = Arc::new(channel.to_string());
    let mut user = discord_user::DiscordUser::new(&token);

    // MESSAGE_CREATE — print and persist.
    {
        let ch = Arc::clone(&target_channel);
        let db = Arc::clone(&db);
        let meta = meta.clone();
        let _guard = user
            .on_message_create(move |event| {
                let msg = &event.message;
                if msg.channel_id != *ch {
                    return;
                }
                let ts = DateTime::parse_from_rfc3339(&msg.timestamp)
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|_| msg.timestamp.clone());
                let sender = msg
                    .author
                    .global_name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&msg.author.username);
                let display = msg.content.replace('\n', " ");
                println!("{} {}: {}", ts.dimmed(), sender.bold(), display);

                // Persist via the same DTO path the REST sync uses, so the
                // schema and edit semantics stay consistent.
                let raw = crate::types::MessageRaw {
                    id: msg.id.clone(),
                    channel_id: Some(msg.channel_id.clone()),
                    content: msg.content.clone(),
                    timestamp: msg.timestamp.clone(),
                    edited_timestamp: msg.edited_timestamp.clone(),
                    author: crate::types::AuthorDto {
                        id: msg.author.id.clone(),
                        username: msg.author.username.clone(),
                        global_name: msg.author.global_name.clone(),
                    },
                    attachments: msg
                        .attachments
                        .iter()
                        .map(|a| crate::types::AttachmentDto {
                            id: a.id.clone(),
                            filename: a.filename.clone(),
                            url: Some(a.url.clone()),
                            content_type: a.content_type.clone(),
                            size: a.size,
                        })
                        .collect(),
                    embeds: Vec::new(),
                };
                let stored = crate::types::StoredMessage::from_raw_with_ctx(
                    &raw,
                    &msg.channel_id,
                    &meta,
                );
                let db = Arc::clone(&db);
                tokio::spawn(async move {
                    if let Err(e) = db.lock().await.insert_batch(&[stored]) {
                        output::err(&format!("persist MESSAGE_CREATE failed: {}", e));
                    }
                });
            })
            .await;
        // Leak the guard — it must outlive the gateway connection. The
        // Tokio runtime will be torn down at process exit.
        std::mem::forget(_guard);
    }

    // MESSAGE_UPDATE — print and persist edit.
    {
        let ch = Arc::clone(&target_channel);
        let db = Arc::clone(&db);
        let _guard = user
            .on_message_update(move |event| {
                if event.channel_id != *ch {
                    return;
                }
                let Some(content) = event.content.as_deref() else {
                    return;
                };
                let display = content.replace('\n', " ");
                println!(
                    "{} {} {}: {}",
                    "          ".dimmed(),
                    event.id.dimmed(),
                    "[edited]".yellow(),
                    display
                );
                let id = event.id.clone();
                let new_content = content.to_string();
                let edited_at = event.edited_timestamp.clone();
                let db = Arc::clone(&db);
                tokio::spawn(async move {
                    if let Err(e) = db
                        .lock()
                        .await
                        .apply_edit(&id, Some(&new_content), edited_at.as_deref())
                    {
                        output::err(&format!("persist MESSAGE_UPDATE failed: {}", e));
                    }
                });
            })
            .await;
        std::mem::forget(_guard);
    }

    // MESSAGE_DELETE — annotate the stored row.
    {
        let ch = Arc::clone(&target_channel);
        let db = Arc::clone(&db);
        let _guard = user
            .on_message_delete(move |event| {
                if event.channel_id != *ch {
                    return;
                }
                println!(
                    "{} {} {}",
                    "          ".dimmed(),
                    event.id.dimmed(),
                    "[deleted]".red()
                );
                let id = event.id.clone();
                let db = Arc::clone(&db);
                tokio::spawn(async move {
                    if let Err(e) = db.lock().await.apply_delete(&id) {
                        output::err(&format!("persist MESSAGE_DELETE failed: {}", e));
                    }
                });
            })
            .await;
        std::mem::forget(_guard);
    }

    user.init()
        .await
        .map_err(|e| anyhow!("Gateway connection failed: {}", e))?;

    // Resume snapshot from last_msg_id. We hold the lock briefly; the
    // gateway callbacks each await it independently.
    let last = db.lock().await.last_msg_id(channel)?;
    let snapshot_label = if last.is_some() { "since last sync" } else { "recent" };
    let initial = api
        .fetch_messages_page(channel, last.as_deref(), None, limit, &meta)
        .await?;
    if !initial.messages.is_empty() {
        let _ = db.lock().await.insert_batch(&initial.messages)?;
        output::dim(&format!(
            "--- {} ({} messages) ---",
            snapshot_label,
            initial.messages.len()
        ));
        for m in &initial.messages {
            println!("{}", output::format_message(m));
        }
        output::dim("--- live stream ---");
    }

    output::dim("Connected. Streaming messages, edits, and deletes...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| anyhow!("failed to install Ctrl+C handler: {}", e))?;
    output::dim("\nDisconnecting...");
    user.disconnect().await;
    output::dim("Stopped.");
    Ok(())
}
