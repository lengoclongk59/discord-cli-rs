//! `discord dc sync-all [-n N]` — discover guilds + channels and sync each.

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::db::Db;
use crate::output;
use crate::types::{ChannelContext, ChannelDto, ThreadDto};

pub async fn run(ctx: &Ctx, limit: u32) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);
    let mut db = Db::open(&ctx.db_path)?;

    let guilds = api.list_guilds().await?;
    output::dim(&format!(
        "Discovered {} guilds. Listing channels...",
        guilds.len()
    ));

    // Pre-size for the common case of multiple channels per guild.
    let mut all_channels: Vec<(String, String, ChannelDto)> = Vec::with_capacity(guilds.len() * 8);
    for g in &guilds {
        match api.list_text_channels(&g.id).await {
            Ok(channels) => {
                let gid = g.id.clone();
                let gname = g.name.clone();
                for ch in channels {
                    all_channels.push((gid.clone(), gname.clone(), ch));
                }
            }
            Err(e) => {
                output::err(&format!("{} (channels): {}", g.name, e));
            }
        }
    }

    // Also discover active threads.
    let mut all_threads: Vec<(String, String, ThreadDto, String)> =
        Vec::with_capacity(guilds.len() * 4);
    for g in &guilds {
        match api.get_active_threads(&g.id).await {
            Ok(resp) => {
                let gid = g.id.clone();
                let gname = g.name.clone();
                for t in resp.threads {
                    let name = t.name.clone().unwrap_or_else(|| t.id.clone());
                    all_threads.push((gid.clone(), gname.clone(), t, name));
                }
            }
            Err(e) => {
                output::err(&format!("{} (threads): {}", g.name, e));
            }
        }
    }

    // Batched lookup of last_msg_id for every channel + thread — replaces
    // a per-channel SELECT round-trip with a single grouped query.
    let mut all_ids: Vec<&str> = Vec::with_capacity(all_channels.len() + all_threads.len());
    for (_, _, ch) in &all_channels {
        all_ids.push(&ch.id);
    }
    for (_, _, t, _) in &all_threads {
        all_ids.push(&t.id);
    }
    let cursors = db.last_msg_ids(&all_ids)?;

    let total_targets = all_channels.len() + all_threads.len();
    let pb = ProgressBar::new(total_targets as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:30.cyan/dim}] {pos}/{len} targets ({msg})")
            .expect("ProgressStyle template is a compile-time constant")
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
        let last = cursors.get(&ch.id);
        match api
            .fetch_messages_page(&ch.id, last.map(|s| s.as_str()), None, limit, &ctx)
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

    for (guild_id, guild_name, t, thread_name) in &all_threads {
        pb.set_message(format!("{} thread #{}", guild_name, thread_name));

        let ctx = ChannelContext {
            guild_id: Some(guild_id.clone()),
            guild_name: Some(guild_name.clone()),
            channel_name: Some(thread_name.clone()),
        };
        let last = cursors.get(&t.id);
        match api
            .fetch_messages_page(&t.id, last.map(|s| s.as_str()), None, limit, &ctx)
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
