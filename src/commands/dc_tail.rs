//! `discord dc tail <CHANNEL>` — stream messages in real time via the
//! Gateway, persisting MESSAGE_CREATE/UPDATE/DELETE into the local archive.
//!
//! Snapshot policy: the initial REST fetch starts from `last_msg_id` so a
//! restart no longer re-prints (and re-stores) the same `--limit` window.
//! When the DB is empty we fall back to the most recent `--limit`. The
//! snapshot fetch runs **before** `user.init()` so the live callback queue
//! and the snapshot rows don't race for ordering in the writer thread.
//!
//! ## Concurrency model
//!
//! A single dedicated OS thread owns the `Db` connection (rusqlite is sync
//! and `!Sync`; holding a `tokio::Mutex<Db>` across `.await` would serialize
//! the gateway pipeline behind every SQLite transaction — flagged by the
//! lock-ordering rule). Gateway callbacks push `DbCmd` variants onto an
//! `mpsc` channel; the writer drains them. The writer batches consecutive
//! Insert commands into a single transaction (one fsync per batch instead
//! of one per message). Shutdown is coordinated via Ctrl+C; on shutdown we
//! drop the callback guards first (so callbacks stop sending), then send
//! `Shutdown`, then drain.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::DateTime;
use colored::Colorize;
use tokio::sync::mpsc;
use tracing::error;

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::db::Db;
use crate::output;
use crate::types::{ChannelContext, StoredMessage};
use crate::wire_enums::TailMode;

const GATEWAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum Insert commands to fold into a single transaction inside the
/// writer thread. Bounded so a quiet channel doesn't wait for a 32-deep
/// queue to fill.
const INSERT_DRAIN_BURST: usize = 32;

/// Commands sent to the dedicated DB writer thread.
enum DbCmd {
    Insert(StoredMessage),
    Edit {
        msg_id: String,
        new_content: String,
        edited_at: Option<String>,
    },
    Delete {
        msg_id: String,
    },
    Shutdown,
}

pub async fn run(ctx: &Ctx, channel: &str, limit: u32, once: bool) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);

    match TailMode::from_once_flag(once) {
        TailMode::Snapshot => {
            let messages = api.fetch_messages(channel, None, limit).await?;
            for m in &messages {
                println!("{}", output::format_message(m));
            }
            Ok(())
        }
        TailMode::Stream => run_stream(ctx, &token, channel, limit, &api).await,
    }
}

async fn run_stream(
    ctx: &Ctx,
    token: &str,
    channel: &str,
    limit: u32,
    api: &Api,
) -> Result<()> {
    let meta = api
        .resolve_channel_context(channel)
        .await
        .unwrap_or_else(|e| {
            output::warn(&format!("could not resolve channel context: {}", e));
            ChannelContext::default()
        });

    // Snapshot BEFORE the gateway is live, so we don't race live-event
    // callbacks against the snapshot rows. Use a short-lived blocking task
    // to read `last_msg_id` without holding a tokio Mutex.
    let db_path_snap = ctx.db_path.clone();
    let channel_owned = channel.to_string();
    let last: Option<String> = tokio::task::spawn_blocking(move || -> Result<Option<String>> {
        let db = Db::open(&db_path_snap)?;
        db.last_msg_id(&channel_owned)
    })
    .await
    .context("spawn_blocking for last_msg_id panicked")??;

    let snapshot_label = if last.is_some() { "since last sync" } else { "recent" };
    let initial = api
        .fetch_messages_page(channel, last.as_deref(), None, limit, &meta)
        .await?;

    // Now spawn the dedicated DB writer thread.
    let db_path = ctx.db_path.clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<DbCmd>();
    let writer = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut db = Db::open(&db_path)?;
        let mut burst: Vec<StoredMessage> = Vec::with_capacity(INSERT_DRAIN_BURST);

        // Helper to flush any pending inserts as a single transaction.
        let flush_inserts =
            |db: &mut Db, burst: &mut Vec<StoredMessage>| {
                if burst.is_empty() {
                    return;
                }
                if let Err(e) = db.insert_batch(burst) {
                    error!(error = %e, count = burst.len(), "persist Insert batch failed");
                }
                burst.clear();
            };

        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                DbCmd::Insert(m) => {
                    burst.push(m);
                    // Opportunistically drain more pending Inserts so we
                    // commit them as one transaction. Stop on the first
                    // non-Insert variant — preserve relative order vs
                    // edits/deletes.
                    while burst.len() < INSERT_DRAIN_BURST {
                        match rx.try_recv() {
                            Ok(DbCmd::Insert(more)) => burst.push(more),
                            Ok(other) => {
                                flush_inserts(&mut db, &mut burst);
                                handle_non_insert(&mut db, other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    if burst.len() >= INSERT_DRAIN_BURST || rx.is_empty() {
                        flush_inserts(&mut db, &mut burst);
                    }
                }
                other => {
                    flush_inserts(&mut db, &mut burst);
                    if matches!(other, DbCmd::Shutdown) {
                        break;
                    }
                    handle_non_insert(&mut db, other);
                }
            }
        }
        flush_inserts(&mut db, &mut burst);
        Ok(())
    });

    // Forward the snapshot rows to the writer thread BEFORE registering
    // gateway callbacks — guarantees historical rows reach the DB first.
    if !initial.messages.is_empty() {
        output::dim(&format!(
            "--- {} ({} messages) ---",
            snapshot_label,
            initial.messages.len()
        ));
        for m in &initial.messages {
            println!("{}", output::format_message(m));
            if tx.send(DbCmd::Insert(m.clone())).is_err() {
                error!("DB writer channel closed during initial snapshot");
                break;
            }
        }
        output::dim("--- live stream ---");
    }

    output::dim(&format!(
        "Connecting to Gateway for channel {}... press Ctrl+C to stop",
        channel
    ));

    let target_channel: Arc<str> = Arc::from(channel);
    let mut user = discord_user::DiscordUser::new(token);

    // MESSAGE_CREATE — print and persist.
    let create_guard = {
        let ch = Arc::clone(&target_channel);
        let tx_cb = tx.clone();
        let meta = meta.clone();
        user.on_message_create(move |event| {
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
            let stored = match StoredMessage::try_from_raw_with_ctx(&raw, &msg.channel_id, &meta) {
                Ok(s) => s,
                Err(e) => {
                    error!(error = %e, msg_id = %raw.id, "drop unstorable MESSAGE_CREATE");
                    return;
                }
            };
            if tx_cb.send(DbCmd::Insert(stored)).is_err() {
                error!("DB writer channel closed; dropping MESSAGE_CREATE");
            }
        })
        .await
    };

    // MESSAGE_UPDATE.
    let update_guard = {
        let ch = Arc::clone(&target_channel);
        let tx_cb = tx.clone();
        user.on_message_update(move |event| {
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
            if tx_cb
                .send(DbCmd::Edit {
                    msg_id: event.id.clone(),
                    new_content: content.to_string(),
                    edited_at: event.edited_timestamp.clone(),
                })
                .is_err()
            {
                error!("DB writer channel closed; dropping MESSAGE_UPDATE");
            }
        })
        .await
    };

    // MESSAGE_DELETE.
    let delete_guard = {
        let ch = Arc::clone(&target_channel);
        let tx_cb = tx.clone();
        user.on_message_delete(move |event| {
            if event.channel_id != *ch {
                return;
            }
            println!(
                "{} {} {}",
                "          ".dimmed(),
                event.id.dimmed(),
                "[deleted]".red()
            );
            if tx_cb
                .send(DbCmd::Delete {
                    msg_id: event.id.clone(),
                })
                .is_err()
            {
                error!("DB writer channel closed; dropping MESSAGE_DELETE");
            }
        })
        .await
    };

    // Bound the gateway handshake — WSS connect can hang on misconfigured
    // networks without an explicit ceiling.
    match tokio::time::timeout(GATEWAY_CONNECT_TIMEOUT, user.init()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(anyhow!("Gateway connection failed: {}", e)),
        Err(_) => {
            return Err(anyhow!(
                "Gateway connect timed out after {}s",
                GATEWAY_CONNECT_TIMEOUT.as_secs()
            ))
        }
    }

    output::dim("Connected. Streaming messages, edits, and deletes...");

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| anyhow!("failed to install Ctrl+C handler: {}", e))?;

    output::dim("\nDisconnecting...");
    user.disconnect().await;

    // Shutdown order matters:
    //   1. Drop callback guards → severs the tx_cb clones each closure
    //      captured, so any in-flight callback that fires *after* this
    //      point can no longer enqueue. Without this, late callbacks would
    //      race the Shutdown sentinel and could be dropped on closed-channel.
    //   2. Send Shutdown via our own tx (which is still alive).
    //   3. Drop our tx — all senders now closed.
    //   4. Await writer — it processes the burst-drain and exits.
    drop(create_guard);
    drop(update_guard);
    drop(delete_guard);

    let _ = tx.send(DbCmd::Shutdown);
    drop(tx);
    match writer.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => output::warn(&format!("DB writer thread error: {}", e)),
        Err(e) => output::warn(&format!("DB writer thread panicked: {}", e)),
    }
    output::dim("Stopped.");
    Ok(())
}

/// Apply a single non-Insert command in the writer thread. Inserts are
/// handled inline via the burst-drain optimization.
fn handle_non_insert(db: &mut Db, cmd: DbCmd) {
    match cmd {
        DbCmd::Edit {
            msg_id,
            new_content,
            edited_at,
        } => {
            if let Err(e) = db.apply_edit(&msg_id, Some(&new_content), edited_at.as_deref()) {
                error!(error = %e, "persist MESSAGE_UPDATE failed");
            }
        }
        DbCmd::Delete { msg_id } => {
            if let Err(e) = db.apply_delete(&msg_id) {
                error!(error = %e, "persist MESSAGE_DELETE failed");
            }
        }
        // Shutdown is handled inline by the caller (it breaks the loop).
        // Insert is handled by the burst-drain path.
        DbCmd::Shutdown | DbCmd::Insert(_) => {
            debug_assert!(false, "handle_non_insert called with Insert/Shutdown");
        }
    }
}
