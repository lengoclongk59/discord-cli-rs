//! `discord dc sync-all [-n N]` — discover guilds + channels and sync each.

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::db::Db;
use crate::output;
use crate::types::ChannelContext;

pub async fn run(ctx: &Ctx, limit: u32) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.clone())?;
    let api = Api::new(&token);
    let mut db = Db::open(&ctx.db_path)?;

    let guilds = api.list_guilds().await?;
    output::dim(&format!(
        "Discovered {} guilds. Listing channels...",
        guilds.len()
    ));

    // Count total channels for progress bar
    let mut all_channels = Vec::new();
    for g in &guilds {
        match api.list_text_channels(&g.id).await {
            Ok(channels) => {
                for ch in channels {
                    all_channels.push((g.id.clone(), g.name.clone(), ch));
                }
            }
            Err(e) => {
                output::err(&format!("{} (channels): {}", g.name, e));
            }
        }
    }

    // Also discover active threads
    let mut all_threads = Vec::new();
    for g in &guilds {
        match api.get_active_threads(&g.id).await {
            Ok(resp) => {
                for t in resp.threads {
                    let name = t.name.clone().unwrap_or_else(|| t.id.clone());
                    all_threads.push((g.id.clone(), g.name.clone(), t.id.clone(), name));
                }
            }
            Err(e) => {
                output::err(&format!("{} (threads): {}", g.name, e));
            }
        }
    }

    let total_targets = all_channels.len() + all_threads.len();
    let pb = ProgressBar::new(total_targets as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:30.cyan/dim}] {pos}/{len} targets ({msg})")
            .unwrap()
            .progress_chars("##-"),
    );

    let mut total_new = 0usize;
    let mut hit_limit_targets: Vec<String> = Vec::new();

    for (guild_id, guild_name, ch) in &all_channels {
        let ch_name = ch.name.clone().unwrap_or_else(|| ch.id.clone());
        pb.set_message(format!("{} #{}", guild_name, ch_name));

        let ctx = ChannelContext {
            guild_id: Some(guild_id.clone()),
            guild_name: Some(guild_name.clone()),
            channel_name: Some(ch_name.clone()),
        };
        let last = db.last_msg_id(&ch.id)?;
        match api
            .fetch_messages_page(&ch.id, last.as_deref(), None, limit, &ctx)
            .await
        {
            Ok(page) => {
                let inserted = db.insert_batch(&page.messages)?;
                total_new += inserted;
                if page.hit_limit {
                    hit_limit_targets.push(format!("{} #{}", guild_name, ch_name));
                }
            }
            Err(e) => {
                pb.suspend(|| {
                    output::err(&format!("{} #{}: {}", guild_name, ch_name, e));
                });
            }
        }
        pb.inc(1);
    }

    // Sync threads (same progress bar)
    for (guild_id, guild_name, thread_id, thread_name) in &all_threads {
        pb.set_message(format!("{} thread #{}", guild_name, thread_name));

        let ctx = ChannelContext {
            guild_id: Some(guild_id.clone()),
            guild_name: Some(guild_name.clone()),
            channel_name: Some(thread_name.clone()),
        };
        let last = db.last_msg_id(thread_id)?;
        match api
            .fetch_messages_page(thread_id, last.as_deref(), None, limit, &ctx)
            .await
        {
            Ok(page) => {
                let inserted = db.insert_batch(&page.messages)?;
                total_new += inserted;
                if page.hit_limit {
                    hit_limit_targets.push(format!("{} thread #{}", guild_name, thread_name));
                }
            }
            Err(e) => {
                pb.suspend(|| {
                    output::err(&format!("{} thread #{}: {}", guild_name, thread_name, e));
                });
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    output::success(&format!(
        "Synced {} new messages across {} channels + {} threads in {} guilds",
        total_new,
        all_channels.len(),
        all_threads.len(),
        guilds.len()
    ));
    if !hit_limit_targets.is_empty() {
        output::warn(&format!(
            "{} target(s) hit the per-target limit ({}); re-run with `-n` larger or repeat to continue: {}",
            hit_limit_targets.len(),
            limit,
            hit_limit_targets.join(", ")
        ));
    }
    Ok(())
}
