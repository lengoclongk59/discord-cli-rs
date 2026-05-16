//! Local DTOs for the Discord JSON we consume + the SQLite row type.
//!
//! Insulates the binary from upstream `discord_user::types` churn — we only
//! deserialize the fields we actually use.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `GET /users/@me` response (fields we use).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MeResponse {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    #[serde(default)]
    pub mfa_enabled: bool,
    #[serde(default)]
    pub premium_type: u32,
    pub email: Option<String>,
    pub phone: Option<String>,
}

/// `GET /users/@me/guilds` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuildSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner: bool,
}

/// `GET /guilds/{id}/channels` row (subset).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)] // parent_id is captured for future thread-vs-channel disambiguation
pub struct ChannelDto {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_: u32,
    #[serde(default)]
    pub position: i32,
    pub parent_id: Option<String>,
    pub topic: Option<String>,
}

/// `GET /channels/{id}/messages` row (subset). Discord returns messages
/// newest-first; we re-sort ascending by snowflake before insert.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // channel_id mirrors API; we use the path param instead
pub struct MessageRaw {
    pub id: String,
    pub channel_id: Option<String>,
    #[serde(default)]
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    pub author: AuthorDto,
    #[serde(default)]
    pub attachments: Vec<AttachmentDto>,
    #[serde(default)]
    pub embeds: Vec<EmbedDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthorDto {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttachmentDto {
    pub id: String,
    #[serde(default = "default_filename")]
    pub filename: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: u64,
}

fn default_filename() -> String {
    "file".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbedDto {
    pub title: Option<String>,
}

/// One row of the `attachments` table — keyed by `(msg_id, attach_id)`.
#[derive(Debug, Clone, Serialize)]
pub struct StoredAttachment {
    pub attach_id: String,
    pub filename: String,
    pub url: String,
    pub content_type: Option<String>,
    pub size: u64,
}

/// Caller-provided context used to populate `guild_*` / `channel_name` at
/// write time. Centralises what was previously left as `None` in many call
/// sites.
#[derive(Debug, Clone, Default)]
pub struct ChannelContext {
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    pub channel_name: Option<String>,
}

/// SQLite row.
#[derive(Debug, Clone, Serialize)]
pub struct StoredMessage {
    pub msg_id: String,
    pub channel_id: String,
    pub sender_id: Option<String>,
    pub sender_name: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    pub channel_name: Option<String>,
    pub edited_timestamp: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<StoredAttachment>,
}

impl StoredMessage {
    /// Convert a raw API message into the SQLite row form used by `Db`.
    /// Falls back to `None` for guild/channel name; callers that have
    /// resolved the parent guild should pass a populated `ChannelContext`
    /// via `try_from_raw_with_ctx`.
    pub fn try_from_raw(raw: &MessageRaw, channel_id: &str) -> Result<Self> {
        Self::try_from_raw_with_ctx(raw, channel_id, &ChannelContext::default())
    }

    /// Validates the RFC3339 timestamp on the wire — a malformed timestamp
    /// is a real corruption signal and surfaces as `Err` rather than
    /// silently degrading to epoch (which would poison every chronological
    /// query in the archive).
    pub fn try_from_raw_with_ctx(
        raw: &MessageRaw,
        channel_id: &str,
        ctx: &ChannelContext,
    ) -> Result<Self> {
        let mut content_parts: Vec<String> =
            Vec::with_capacity(1 + raw.attachments.len() + raw.embeds.len());
        if !raw.content.is_empty() {
            content_parts.push(raw.content.clone());
        }
        for att in &raw.attachments {
            content_parts.push(format!("[attachment: {}]", att.filename));
        }
        for embed in &raw.embeds {
            if let Some(title) = &embed.title {
                content_parts.push(format!("[embed: {}]", title));
            }
        }
        let content = content_parts.join("\n");
        let timestamp = DateTime::parse_from_rfc3339(&raw.timestamp)
            .map(|t| t.with_timezone(&Utc))
            .with_context(|| format!("invalid timestamp on message {}: {:?}", raw.id, raw.timestamp))?;

        let sender_name = raw
            .author
            .global_name
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| raw.author.username.clone());

        let attachments = raw
            .attachments
            .iter()
            .filter_map(|a| {
                let url = a.url.clone()?;
                Some(StoredAttachment {
                    attach_id: a.id.clone(),
                    filename: a.filename.clone(),
                    url,
                    content_type: a.content_type.clone(),
                    size: a.size,
                })
            })
            .collect();

        Ok(StoredMessage {
            msg_id: raw.id.clone(),
            channel_id: channel_id.to_string(),
            sender_id: Some(raw.author.id.clone()),
            sender_name,
            content,
            timestamp,
            guild_id: ctx.guild_id.clone(),
            guild_name: ctx.guild_name.clone(),
            channel_name: ctx.channel_name.clone(),
            edited_timestamp: raw.edited_timestamp.clone(),
            attachments,
        })
    }
}

/// `GET /guilds/{id}?with_counts=true` response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuildDetailDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Option<String>,
    #[serde(default)]
    pub approximate_member_count: Option<u64>,
    #[serde(default)]
    pub approximate_presence_count: Option<u64>,
    pub preferred_locale: Option<String>,
    #[serde(default)]
    pub premium_tier: u32,
    #[serde(default)]
    pub premium_subscription_count: Option<u64>,
    pub icon: Option<String>,
    pub banner: Option<String>,
}

/// `GET /guilds/{id}/members` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuildMemberDto {
    pub user: Option<AuthorDto>,
    pub nick: Option<String>,
    #[serde(default)]
    pub joined_at: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Wrapper for the search endpoint's nested response.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    pub messages: Vec<Vec<MessageRaw>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub total_results: u64,
}

/// `GET /guilds/{id}/threads/active` response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActiveThreadsResponse {
    #[serde(default)]
    pub threads: Vec<ThreadDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThreadDto {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub type_: u32,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub message_count: Option<u64>,
    #[serde(default)]
    pub member_count: Option<u64>,
    pub owner_id: Option<String>,
    #[serde(default)]
    pub thread_metadata: Option<ThreadMetadataDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThreadMetadataDto {
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub auto_archive_duration: Option<u32>,
}

/// `GET /users/@me/relationships` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelationshipDto {
    pub id: String,
    #[serde(rename = "type", default)]
    pub type_: u8,
    pub user: Option<RelationshipUserDto>,
    pub nickname: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelationshipUserDto {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
}

/// `GET /guilds/{id}/roles` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoleDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub hoist: bool,
    #[serde(default)]
    pub position: i32,
    #[serde(default)]
    pub permissions: Option<String>,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub mentionable: bool,
}

/// `GET /guilds/{id}/emojis` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmojiDto {
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub require_colons: bool,
}

/// `GET /users/{id}/profile` response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserProfileDto {
    pub user: Option<AuthorDto>,
    #[serde(default)]
    pub connected_accounts: Vec<ConnectedAccountDto>,
    pub premium_since: Option<String>,
    #[serde(default)]
    pub premium_type: Option<u32>,
    pub user_profile: Option<UserProfileDataDto>,
    #[serde(default)]
    pub mutual_guilds: Vec<MutualGuildDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConnectedAccountDto {
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserProfileDataDto {
    pub bio: Option<String>,
    pub pronouns: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MutualGuildDto {
    pub id: Option<String>,
    pub nick: Option<String>,
}

/// `GET /guilds/{id}/stickers` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StickerDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub format_type: u32,
    #[serde(default)]
    pub available: bool,
}

/// `GET /guilds/{id}/audit-logs` response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuditLogResponse {
    #[serde(default)]
    pub audit_log_entries: Vec<AuditLogEntryDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuditLogEntryDto {
    pub id: String,
    pub user_id: Option<String>,
    #[serde(default)]
    pub action_type: u32,
    pub target_id: Option<String>,
    pub reason: Option<String>,
}

/// `GET /guilds/{id}/scheduled-events` row.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScheduledEventDto {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub scheduled_start_time: Option<String>,
    pub scheduled_end_time: Option<String>,
    #[serde(default)]
    pub status: u32,
    #[serde(default)]
    pub entity_type: u32,
    #[serde(default)]
    pub user_count: Option<u64>,
}

/// Full server snapshot for JSON export.
#[derive(Debug, Clone, Serialize)]
pub struct ServerSnapshot {
    pub guild: GuildDetailDto,
    pub channels: Vec<ChannelDto>,
    pub roles: Vec<RoleDto>,
    pub emojis: Vec<EmojiDto>,
    pub stickers: Vec<StickerDto>,
    pub members: Vec<GuildMemberDto>,
    pub threads: Vec<ThreadDto>,
}
