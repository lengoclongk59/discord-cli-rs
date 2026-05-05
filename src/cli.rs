//! clap CLI definitions.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Clone, Debug, ValueEnum)]
pub enum TimelineGrouping {
    Day,
    Hour,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum ExportFormat {
    Text,
    Json,
    Yaml,
}

#[derive(Parser, Debug)]
#[command(name = "discord", version, about = "Local-first Discord archival CLI")]
pub struct Cli {
    /// Override token resolution (flag wins over env and .env)
    #[arg(long, global = true)]
    pub token: Option<String>,

    /// Override SQLite database path
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Disable ANSI color output
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Output results as JSON
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Auto-discover and (optionally) save the Discord user token
    Auth(AuthArgs),
    /// Verify the configured token works
    Status,
    /// Show profile of the authenticated user
    Whoami,
    /// Discord API operations (guilds, channels, sync, ...)
    #[command(subcommand)]
    Dc(DcCmd),
    /// Search the local message archive
    Search(SearchArgs),
    /// Show newest stored messages
    Recent(RecentArgs),
    /// Per-channel message counts
    Stats,
    /// Show today's messages grouped by channel
    Today(TodayArgs),
    /// Show most active senders
    Top(TopArgs),
    /// Show message activity over time
    Timeline(TimelineArgs),
    /// Export messages to file
    Export(ExportArgs),
    /// Delete stored messages for a channel
    Purge(PurgeArgs),
}

#[derive(Args, Debug)]
pub struct AuthArgs {
    /// Save the discovered token to `./.env`
    #[arg(long)]
    pub save: bool,
}

#[derive(Subcommand, Debug)]
pub enum DcCmd {
    /// List joined guilds
    Guilds,
    /// List text channels in a guild (id or name)
    Channels {
        /// Guild ID (digits) or partial guild name
        guild: String,
    },
    /// Incremental sync: fetch only new messages
    Sync {
        /// Channel ID
        channel: String,
        /// Maximum messages to fetch
        #[arg(short = 'n', long, default_value_t = 5000)]
        limit: u32,
    },
    /// Discover all guilds + text channels and sync each
    SyncAll {
        /// Maximum messages per channel
        #[arg(short = 'n', long, default_value_t = 200)]
        limit: u32,
    },
    /// Fetch historical messages backward into local DB
    History {
        /// Channel ID
        channel: String,
        /// Maximum messages to fetch
        #[arg(short = 'n', long, default_value_t = 1000)]
        limit: u32,
    },
    /// Stream a channel's messages in real time via Gateway WebSocket
    Tail {
        /// Channel ID
        channel: String,
        /// Initial snapshot size
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
        /// Fetch once then exit (no Gateway connection)
        #[arg(long)]
        once: bool,
    },
    /// Search messages on Discord's server (remote search)
    #[command(name = "search")]
    SearchRemote {
        /// Guild ID or partial guild name
        guild: String,
        /// Search keyword
        keyword: String,
        /// Filter by channel ID
        #[arg(short = 'c', long)]
        channel: Option<String>,
        /// Maximum results
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: u32,
    },
    /// List guild members
    Members {
        /// Guild ID or partial guild name
        guild: String,
        /// Maximum members to fetch
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u32,
    },
    /// Show detailed guild information
    Info {
        /// Guild ID or partial guild name
        guild: String,
    },
    /// Show pinned messages in a channel
    Pins {
        /// Channel ID
        channel: String,
    },
    /// List active threads in a guild
    Threads {
        /// Guild ID or partial guild name
        guild: String,
    },
    /// Show relationships (friends, blocked, pending)
    Relationships,
    /// List roles in a guild
    Roles {
        /// Guild ID or partial guild name
        guild: String,
    },
    /// List custom emojis in a guild
    Emojis {
        /// Guild ID or partial guild name
        guild: String,
    },
    /// Show a user's profile
    Profile {
        /// User ID (defaults to self)
        user_id: Option<String>,
    },
    /// List custom stickers in a guild
    Stickers {
        /// Guild ID or partial guild name
        guild: String,
    },
    /// Show recent audit log entries
    Audit {
        /// Guild ID or partial guild name
        guild: String,
        /// Maximum entries
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u8,
    },
    /// List scheduled events in a guild
    Events {
        /// Guild ID or partial guild name
        guild: String,
    },
    /// Download attachments from stored messages
    Download {
        /// Channel ID or local channel name
        channel: String,
        /// Output directory
        #[arg(short = 'o', long, default_value = "downloads")]
        output: PathBuf,
        /// Only from the last N hours
        #[arg(long)]
        hours: Option<i64>,
    },
    /// Export full guild snapshot as JSON
    Snapshot {
        /// Guild ID or partial guild name
        guild: String,
        /// Output file (stdout if omitted)
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Search keyword (case-insensitive substring)
    pub keyword: String,
    /// Filter by channel ID or local channel name
    #[arg(short = 'c', long)]
    pub channel: Option<String>,
    /// Maximum results
    #[arg(short = 'n', long, default_value_t = 50)]
    pub limit: i64,
}

#[derive(Args, Debug)]
pub struct RecentArgs {
    /// Filter by channel ID or local channel name
    #[arg(short = 'c', long)]
    pub channel: Option<String>,
    /// Only messages from the last N hours
    #[arg(long)]
    pub hours: Option<i64>,
    /// Maximum results
    #[arg(short = 'n', long, default_value_t = 50)]
    pub limit: i64,
}

#[derive(Args, Debug)]
pub struct TodayArgs {
    /// Filter by channel ID or local channel name
    #[arg(short = 'c', long)]
    pub channel: Option<String>,
}

#[derive(Args, Debug)]
pub struct TopArgs {
    /// Filter by channel ID or local channel name
    #[arg(short = 'c', long)]
    pub channel: Option<String>,
    /// Only messages from the last N hours
    #[arg(long)]
    pub hours: Option<i64>,
    /// Maximum results
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: i64,
}

#[derive(Args, Debug)]
pub struct TimelineArgs {
    /// Filter by channel ID or local channel name
    #[arg(short = 'c', long)]
    pub channel: Option<String>,
    /// Only messages from the last N hours
    #[arg(long)]
    pub hours: Option<i64>,
    /// Group by "day" or "hour"
    #[arg(long, default_value = "day", value_enum)]
    pub by: TimelineGrouping,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Channel ID or local channel name
    pub channel: String,
    /// Output format: "text" or "json"
    #[arg(short = 'f', long, default_value = "text", value_enum)]
    pub format: ExportFormat,
    /// Output file (stdout if omitted)
    #[arg(short = 'o', long)]
    pub output: Option<PathBuf>,
    /// Only messages from the last N hours
    #[arg(long)]
    pub hours: Option<i64>,
}

#[derive(Args, Debug)]
pub struct PurgeArgs {
    /// Channel ID or local channel name
    pub channel: String,
    /// Skip confirmation prompt
    #[arg(short = 'y', long)]
    pub yes: bool,
}
