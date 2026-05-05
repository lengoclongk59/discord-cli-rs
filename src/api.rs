//! Read-only Discord REST wrapper.
//!
//! Wraps `discord_user::client::DiscordHttpClient` in a `ReadOnlyHttp` newtype
//! that exposes only `.get()`. The read-only invariant of this binary is
//! enforced at the type level: there is no path from `Api` to a write verb.
//!
//! The single sanctioned exception is `auth::windows::validate_token`, which
//! uses raw `reqwest` directly (NOT through this module) because it must run
//! before a token exists in the resolution chain. The `tests/readonly.rs`
//! grep test allowlists that file explicitly.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use rand::Rng;
use serde::de::DeserializeOwned;
use tracing::debug;

use discord_user::client::DiscordHttpClient;
use discord_user::route::Route;

use crate::types::{
    ActiveThreadsResponse, AuditLogResponse, ChannelContext, ChannelDto, EmojiDto, GuildDetailDto,
    GuildMemberDto, GuildSummary, MeResponse, MessageRaw, RelationshipDto, RoleDto,
    ScheduledEventDto, SearchResponse, StickerDto, StoredMessage, UserProfileDto,
};

/// Newtype that exposes ONLY `.get<T>()` from `DiscordHttpClient`.
/// Makes the read-only invariant a compile-time guarantee.
pub struct ReadOnlyHttp {
    inner: DiscordHttpClient,
}

impl ReadOnlyHttp {
    pub fn new(token: &str) -> Self {
        Self {
            inner: DiscordHttpClient::new(token, None, false),
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, route: Route<'_>) -> Result<T> {
        self.inner
            .get::<T>(route)
            .await
            .map_err(|e| anyhow!(e.to_string()))
    }
}

/// Parse a Discord snowflake (string of digits) into a u64. Used at the
/// SQLite-TEXT ↔ `Route::GetMessages`-u64 boundary.
pub fn parse_snowflake(s: &str) -> Result<u64> {
    s.parse::<u64>()
        .with_context(|| format!("invalid snowflake id: {:?}", s))
}

/// Result of a paginated message fetch.
///
/// `hit_limit` is true when the loop exited because `limit` was reached
/// rather than because Discord returned a short page — callers should
/// surface this so users know to re-run with a larger `-n`.
pub struct FetchedPage {
    pub messages: Vec<StoredMessage>,
    pub hit_limit: bool,
    pub oldest_msg_id: Option<String>,
}

pub struct Api {
    http: ReadOnlyHttp,
}

impl Api {
    pub fn new(token: &str) -> Self {
        Self {
            http: ReadOnlyHttp::new(token),
        }
    }

    pub async fn get_me(&self) -> Result<MeResponse> {
        self.http.get::<MeResponse>(Route::GetMe).await
    }

    pub async fn list_guilds(&self) -> Result<Vec<GuildSummary>> {
        self.http
            .get::<Vec<GuildSummary>>(Route::GetCurrentUserGuilds)
            .await
    }

    /// Lists text-class channels (types 0/5/15) sorted by `position`.
    pub async fn list_text_channels(&self, guild_id: &str) -> Result<Vec<ChannelDto>> {
        let gid = parse_snowflake(guild_id)?;
        let mut chans: Vec<ChannelDto> = self
            .http
            .get::<Vec<ChannelDto>>(Route::GetGuildChannels { guild_id: gid })
            .await?;
        chans.retain(|c| matches!(c.type_, 0 | 5 | 15));
        chans.sort_by_key(|c| c.position);
        Ok(chans)
    }

    /// Paginated message fetch with jittered sleep between pages.
    /// `after` is a snowflake string; converted to u64 at the boundary.
    pub async fn fetch_messages(
        &self,
        channel_id: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredMessage>> {
        let page = self
            .fetch_messages_page(channel_id, after, None, limit, &ChannelContext::default())
            .await?;
        Ok(page.messages)
    }

    /// Paginated fetch with optional `before` cursor (for backfill) and
    /// caller-supplied channel context (so rows arrive pre-annotated).
    pub async fn fetch_messages_page(
        &self,
        channel_id: &str,
        after: Option<&str>,
        before: Option<&str>,
        limit: u32,
        ctx: &ChannelContext,
    ) -> Result<FetchedPage> {
        let cid = parse_snowflake(channel_id)?;
        let mut all: Vec<StoredMessage> = Vec::new();
        let mut remaining = limit;
        let mut after_cursor: Option<u64> = after.map(parse_snowflake).transpose()?;
        let mut before_cursor: Option<u64> = before.map(parse_snowflake).transpose()?;
        let use_after = after_cursor.is_some();
        let mut hit_limit = false;
        let mut oldest_id: Option<u64> = None;

        while remaining > 0 {
            let batch_limit = remaining.min(100);
            let route = Route::GetMessages {
                channel_id: cid,
                limit: Some(batch_limit),
                before: if use_after { None } else { before_cursor },
                after: after_cursor,
            };
            debug!(
                "fetch_messages: channel={} after={:?} before={:?} limit={}",
                channel_id, after_cursor, before_cursor, batch_limit
            );
            let page: Vec<MessageRaw> = self.http.get::<Vec<MessageRaw>>(route).await?;
            if page.is_empty() {
                break;
            }
            for raw in &page {
                all.push(StoredMessage::from_raw_with_ctx(raw, channel_id, ctx));
            }
            let page_len = page.len() as u32;
            remaining = remaining.saturating_sub(page_len);

            // Discord returns newest-first by default, but when `after=` is
            // supplied it returns oldest-first. Pick numeric extremes
            // regardless of order — these are the safe cursors.
            let max_id_in_page = page.iter().filter_map(|m| m.id.parse::<u64>().ok()).max();
            let min_id_in_page = page.iter().filter_map(|m| m.id.parse::<u64>().ok()).min();
            if let Some(min) = min_id_in_page {
                oldest_id = Some(oldest_id.map_or(min, |o| o.min(min)));
            }

            if (page_len as u32) < batch_limit {
                break;
            }

            if remaining == 0 {
                // We exited because `limit` ran out; tell the caller.
                hit_limit = true;
                break;
            }

            if use_after {
                after_cursor = max_id_in_page;
            } else {
                before_cursor = min_id_in_page;
            }

            // Polite jittered backoff between pages.
            let sleep_ms = rand::rng().random_range(300u64..1000u64);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }

        // Sort ascending by snowflake numeric value before returning.
        all.sort_by_key(|m| m.msg_id.parse::<u128>().unwrap_or(0));
        Ok(FetchedPage {
            messages: all,
            hit_limit,
            oldest_msg_id: oldest_id.map(|n| n.to_string()),
        })
    }

    /// One-shot single-channel fetch — used to populate channel name when
    /// resolving metadata.
    pub async fn get_channel(&self, channel_id: &str) -> Result<ChannelDto> {
        let cid = parse_snowflake(channel_id)?;
        self.http
            .get::<ChannelDto>(Route::GetChannel { channel_id: cid })
            .await
    }

    /// Look up a channel's parent guild + names in one shot. Returns
    /// `Ok(ChannelContext { guild_id: None, .. })` for DMs/group DMs (i.e.
    /// no enclosing guild was found). Errors propagate — callers must NOT
    /// silently `unwrap_or_default()`, otherwise rows lose all metadata.
    ///
    /// Cost is O(guilds × channels). Cache the result per CLI invocation
    /// when calling more than once.
    pub async fn resolve_channel_context(&self, channel_id: &str) -> Result<ChannelContext> {
        let chan = self.get_channel(channel_id).await?;
        let guilds = self.list_guilds().await?;
        for g in &guilds {
            let gid = match parse_snowflake(&g.id) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Don't swallow the per-guild error silently — log it. A
            // missing-permission guild is fine to skip; transport errors
            // should bubble.
            let chans: Vec<ChannelDto> = match self
                .http
                .get::<Vec<ChannelDto>>(Route::GetGuildChannels { guild_id: gid })
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    debug!("skipping guild {} for channel lookup: {}", g.id, e);
                    continue;
                }
            };
            if chans.iter().any(|c| c.id == channel_id) {
                return Ok(ChannelContext {
                    guild_id: Some(g.id.clone()),
                    guild_name: Some(g.name.clone()),
                    channel_name: chan.name.clone(),
                });
            }
        }
        // Found channel but no enclosing guild — DM, group DM, or a guild
        // we no longer have access to.
        Ok(ChannelContext {
            guild_id: None,
            guild_name: None,
            channel_name: chan.name,
        })
    }

    pub async fn get_guild_info(&self, guild_id: &str) -> Result<GuildDetailDto> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<GuildDetailDto>(Route::GetGuild {
                guild_id: gid,
                with_counts: true,
            })
            .await
    }

    pub async fn list_guild_members(
        &self,
        guild_id: &str,
        limit: u32,
    ) -> Result<Vec<GuildMemberDto>> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<Vec<GuildMemberDto>>(Route::GetGuildMembers {
                guild_id: gid,
                limit: limit.min(1000),
            })
            .await
    }

    pub async fn search_guild_messages(
        &self,
        guild_id: &str,
        content: &str,
        channel_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StoredMessage>> {
        let gid = parse_snowflake(guild_id)?;
        let cid = channel_id.map(parse_snowflake).transpose()?;
        let resp: SearchResponse = self
            .http
            .get::<SearchResponse>(Route::SearchGuildMessages {
                guild_id: gid,
                content,
                channel_id: cid,
                limit: Some(limit.min(25)),
            })
            .await?;

        let mut results = Vec::new();
        for group in &resp.messages {
            for raw in group {
                let ch = raw.channel_id.as_deref().unwrap_or("");
                results.push(StoredMessage::from_raw(raw, ch));
            }
        }
        Ok(results)
    }

    pub async fn get_pins(&self, channel_id: &str) -> Result<Vec<MessageRaw>> {
        let cid = parse_snowflake(channel_id)?;
        self.http
            .get::<Vec<MessageRaw>>(Route::GetPins { channel_id: cid })
            .await
    }

    pub async fn get_active_threads(&self, guild_id: &str) -> Result<ActiveThreadsResponse> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<ActiveThreadsResponse>(Route::GetActiveThreads { guild_id: gid })
            .await
    }

    pub async fn get_relationships(&self) -> Result<Vec<RelationshipDto>> {
        self.http
            .get::<Vec<RelationshipDto>>(Route::GetRelationships)
            .await
    }

    pub async fn get_guild_roles(&self, guild_id: &str) -> Result<Vec<RoleDto>> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<Vec<RoleDto>>(Route::GetGuildRoles { guild_id: gid })
            .await
    }

    pub async fn get_guild_emojis(&self, guild_id: &str) -> Result<Vec<EmojiDto>> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<Vec<EmojiDto>>(Route::GetGuildEmojis { guild_id: gid })
            .await
    }

    pub async fn get_user_profile(&self, user_id: &str) -> Result<UserProfileDto> {
        let uid = parse_snowflake(user_id)?;
        self.http
            .get::<UserProfileDto>(Route::GetUserProfile { user_id: uid, guild_id: None })
            .await
    }

    pub async fn get_guild_stickers(&self, guild_id: &str) -> Result<Vec<StickerDto>> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<Vec<StickerDto>>(Route::GetGuildStickers { guild_id: gid })
            .await
    }

    pub async fn get_guild_audit_logs(&self, guild_id: &str, limit: u8) -> Result<AuditLogResponse> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<AuditLogResponse>(Route::GetGuildAuditLogs {
                guild_id: gid,
                user_id: None,
                action_type: None,
                before: None,
                after: None,
                limit: Some(limit.min(100)),
            })
            .await
    }

    pub async fn get_scheduled_events(&self, guild_id: &str) -> Result<Vec<ScheduledEventDto>> {
        let gid = parse_snowflake(guild_id)?;
        self.http
            .get::<Vec<ScheduledEventDto>>(Route::GetGuildScheduledEvents { guild_id: gid })
            .await
    }

    pub async fn resolve_guild_id(&self, guild: &str) -> Result<String> {
        if guild.chars().all(|c| c.is_ascii_digit()) {
            return Ok(guild.to_string());
        }
        let guilds = self.list_guilds().await?;
        let needle = guild.to_lowercase();
        let matches: Vec<_> = guilds
            .iter()
            .filter(|g| g.name.to_lowercase().contains(&needle))
            .collect();
        match matches.len() {
            0 => Err(anyhow!("Guild '{}' not found.", guild)),
            1 => Ok(matches[0].id.clone()),
            n => {
                let names: Vec<String> = matches.iter().map(|g| g.name.clone()).collect();
                Err(anyhow!(
                    "{} guilds match '{}': {}. Use a guild ID instead.",
                    n,
                    guild,
                    names.join(", ")
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snowflake_ok() {
        assert_eq!(
            parse_snowflake("123456789012345678").unwrap(),
            123456789012345678u64
        );
    }

    #[test]
    fn parse_snowflake_err() {
        assert!(parse_snowflake("not-a-number").is_err());
        assert!(parse_snowflake("").is_err());
    }
}
