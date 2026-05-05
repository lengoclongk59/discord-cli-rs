//! Subcommand dispatch.

mod auth;
mod dc_audit;
mod dc_channels;
mod dc_download;
mod dc_emojis;
mod dc_events;
mod dc_guilds;
mod dc_history;
mod dc_info;
mod dc_members;
mod dc_pins;
mod dc_profile;
mod dc_relationships;
mod dc_roles;
mod dc_search_remote;
mod dc_snapshot;
mod dc_stickers;
mod dc_sync;
mod dc_sync_all;
mod dc_tail;
mod dc_threads;
mod export;
mod purge;
mod recent;
pub(crate) mod resolve;
mod search;
mod stats;
mod status;
mod timeline;
mod today;
mod top;
mod whoami;

use std::path::PathBuf;

use anyhow::Result;

use crate::cli::{Cli, Cmd, DcCmd, TimelineGrouping};

pub struct Ctx {
    pub token_flag: Option<String>,
    pub db_path: PathBuf,
    pub json: bool,
}

impl Ctx {
    fn from_cli(cli: &Cli) -> Self {
        Self {
            token_flag: cli.token.clone(),
            db_path: cli
                .db
                .clone()
                .unwrap_or_else(|| PathBuf::from("discord.db")),
            json: cli.json,
        }
    }
}

pub async fn dispatch(cli: Cli) -> Result<()> {
    let ctx = Ctx::from_cli(&cli);
    match cli.cmd {
        Cmd::Auth(args) => auth::run(args).await,
        Cmd::Status => status::run(&ctx).await,
        Cmd::Whoami => whoami::run(&ctx).await,
        Cmd::Dc(DcCmd::Guilds) => dc_guilds::run(&ctx).await,
        Cmd::Dc(DcCmd::Channels { guild }) => dc_channels::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Sync { channel, limit }) => dc_sync::run(&ctx, &channel, limit).await,
        Cmd::Dc(DcCmd::SyncAll { limit }) => dc_sync_all::run(&ctx, limit).await,
        Cmd::Dc(DcCmd::History { channel, limit }) => dc_history::run(&ctx, &channel, limit).await,
        Cmd::Dc(DcCmd::Tail {
            channel,
            limit,
            once,
        }) => dc_tail::run(&ctx, &channel, limit, once).await,
        Cmd::Dc(DcCmd::SearchRemote {
            guild,
            keyword,
            channel,
            limit,
        }) => dc_search_remote::run(&ctx, &guild, &keyword, channel.as_deref(), limit).await,
        Cmd::Dc(DcCmd::Members { guild, limit }) => dc_members::run(&ctx, &guild, limit).await,
        Cmd::Dc(DcCmd::Info { guild }) => dc_info::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Pins { channel }) => dc_pins::run(&ctx, &channel).await,
        Cmd::Dc(DcCmd::Threads { guild }) => dc_threads::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Relationships) => dc_relationships::run(&ctx).await,
        Cmd::Dc(DcCmd::Roles { guild }) => dc_roles::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Emojis { guild }) => dc_emojis::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Profile { user_id }) => dc_profile::run(&ctx, user_id.as_deref()).await,
        Cmd::Dc(DcCmd::Stickers { guild }) => dc_stickers::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Audit { guild, limit }) => dc_audit::run(&ctx, &guild, limit).await,
        Cmd::Dc(DcCmd::Events { guild }) => dc_events::run(&ctx, &guild).await,
        Cmd::Dc(DcCmd::Download { channel, output, hours }) => dc_download::run(&ctx, &channel, &output, hours).await,
        Cmd::Dc(DcCmd::Snapshot { guild, output }) => dc_snapshot::run(&ctx, &guild, output).await,
        Cmd::Search(args) => search::run(&ctx, args),
        Cmd::Recent(args) => recent::run(&ctx, args),
        Cmd::Stats => stats::run(&ctx),
        Cmd::Today(args) => today::run(&ctx, args.channel.as_deref()),
        Cmd::Top(args) => top::run(&ctx, args.channel.as_deref(), args.hours, args.limit),
        Cmd::Timeline(args) => {
            let by = match args.by {
                TimelineGrouping::Day => "day",
                TimelineGrouping::Hour => "hour",
            };
            timeline::run(&ctx, args.channel.as_deref(), args.hours, by)
        }
        Cmd::Export(args) => export::run(&ctx, args),
        Cmd::Purge(args) => purge::run(&ctx, args),
    }
}
