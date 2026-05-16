//! SQLite storage for the Discord message archive.

use std::path::Path;

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, NaiveTime, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::types::StoredMessage;

/// One row of `attachments_for_channel`: `(msg_id, attach_id, filename,
/// url, content_type, size)`. The `dc download` callsite tags this shape
/// once at the boundary instead of leaving a 6-tuple in the signature.
pub type AttachmentRow = (String, String, String, String, Option<String>, i64);

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::init_schema(&conn)?;
        Ok(Db { conn })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Db { conn })
    }

    fn init_schema(c: &Connection) -> Result<()> {
        c.execute_batch("PRAGMA journal_mode=WAL;")?;
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if version < 1 {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS messages (
                    msg_id        TEXT PRIMARY KEY,
                    channel_id    TEXT NOT NULL,
                    sender_id     TEXT,
                    sender_name   TEXT,
                    content       TEXT,
                    timestamp     TEXT NOT NULL,
                    guild_id      TEXT,
                    guild_name    TEXT,
                    channel_name  TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_messages_channel_id ON messages(channel_id);
                CREATE INDEX IF NOT EXISTS idx_messages_timestamp  ON messages(timestamp);
                ",
            )?;
            c.execute_batch("PRAGMA user_version = 1;")?;
        }

        if version < 2 {
            // edited_timestamp tracks Discord's edit field so re-syncs can
            // overwrite stale rows; older rows simply get NULL.
            let has_col = c
                .query_row(
                    "SELECT 1 FROM pragma_table_info('messages') WHERE name = 'edited_timestamp'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .is_some();
            if !has_col {
                c.execute_batch("ALTER TABLE messages ADD COLUMN edited_timestamp TEXT;")?;
            }
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS attachments (
                    msg_id        TEXT NOT NULL,
                    attach_id     TEXT NOT NULL,
                    filename      TEXT,
                    url           TEXT,
                    content_type  TEXT,
                    size          INTEGER,
                    PRIMARY KEY (msg_id, attach_id)
                );
                CREATE INDEX IF NOT EXISTS idx_attachments_msg_id ON attachments(msg_id);
                ",
            )?;
            c.execute_batch("PRAGMA user_version = 2;")?;
        }

        if version < 3 {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )?;
            // FTS5 with external content avoids storing a second copy.
            // SQLite without FTS5 will silently fail this CREATE; we fall
            // back to LIKE in `search` if the virtual table is absent.
            let _ = c.execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts
                    USING fts5(content, content='messages', content_rowid='rowid');
                CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
                END;
                CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                    INSERT INTO messages_fts(messages_fts, rowid, content)
                        VALUES ('delete', old.rowid, old.content);
                END;
                CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
                    INSERT INTO messages_fts(messages_fts, rowid, content)
                        VALUES ('delete', old.rowid, old.content);
                    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
                END;",
            );
            // Best-effort backfill of existing rows. Skipped silently if
            // the virtual table does not exist (FTS5 unavailable).
            let _ = c.execute_batch(
                "INSERT INTO messages_fts(rowid, content)
                    SELECT rowid, content FROM messages
                    WHERE rowid NOT IN (SELECT rowid FROM messages_fts);",
            );
            c.execute_batch("PRAGMA user_version = 3;")?;
        }
        Ok(())
    }

    /// True iff the FTS5 virtual table exists. Cached per call site —
    /// SQLite catalog lookups are cheap. A genuine I/O error here would
    /// also bubble out of every subsequent `query_map` call, so we surface
    /// it via `Result` instead of folding it into a `false`.
    fn has_fts(&self) -> Result<bool> {
        let row: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        Ok(row.is_some())
    }

    /// Returns the maximum (newest) `msg_id` stored for the channel, or `None`
    /// if the channel has no rows yet. Snowflakes sort lexicographically as
    /// long as they are equal-length strings, but to be safe we take the
    /// numerically-max value via `MAX(CAST(msg_id AS INTEGER))`.
    pub fn last_msg_id(&self, channel_id: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT msg_id FROM messages WHERE channel_id = ?1 ORDER BY CAST(msg_id AS INTEGER) DESC LIMIT 1")?;
        let row: Option<String> = stmt
            .query_row(params![channel_id], |r| r.get(0))
            .optional()?;
        Ok(row)
    }

    /// Batched `last_msg_id` for a set of channels. Returns a map from
    /// channel_id → latest stored msg_id (entries absent for channels with
    /// no rows). Replaces a per-channel `last_msg_id` loop in `dc sync-all`
    /// — one round-trip instead of N.
    ///
    /// Semantically identical to calling `last_msg_id` per channel: returns
    /// the **raw TEXT** of the winning row, not a re-stringified i64. This
    /// matters because `MAX(CAST(... AS INTEGER))` would canonicalize the
    /// value (strip leading zeros, collapse non-numeric chars). We use a
    /// correlated subquery so the outer SELECT returns the original
    /// `msg_id` column verbatim.
    pub fn last_msg_ids(&self, channel_ids: &[&str]) -> Result<HashMap<String, String>> {
        if channel_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut placeholders = String::with_capacity(channel_ids.len() * 2);
        for i in 0..channel_ids.len() {
            if i > 0 {
                placeholders.push(',');
            }
            placeholders.push('?');
        }
        let sql = format!(
            "SELECT m.channel_id, m.msg_id
             FROM messages m
             WHERE m.channel_id IN ({ph})
               AND CAST(m.msg_id AS INTEGER) = (
                   SELECT MAX(CAST(msg_id AS INTEGER))
                   FROM messages
                   WHERE channel_id = m.channel_id
               )",
            ph = placeholders
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mapped = stmt.query_map(
            params_from_iter(channel_ids.iter().copied()),
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        let mut out = HashMap::with_capacity(channel_ids.len());
        for row in mapped {
            let (cid, msg_id) = row?;
            out.insert(cid, msg_id);
        }
        Ok(out)
    }

    /// Insert a batch in a transaction. New rows are inserted; existing rows
    /// are updated only when the incoming `edited_timestamp` is strictly
    /// newer than the stored one (so re-syncs propagate edits without
    /// rewriting unchanged content). Returns the number of newly-inserted
    /// rows. Edits do not count toward the return value.
    pub fn insert_batch(&mut self, msgs: &[StoredMessage]) -> Result<usize> {
        if msgs.is_empty() {
            return Ok(0);
        }
        let pre_total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        let tx = self.conn.transaction()?;
        {
            let mut msg_stmt = tx.prepare(
                "INSERT INTO messages
                    (msg_id, channel_id, sender_id, sender_name, content, timestamp,
                     guild_id, guild_name, channel_name, edited_timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(msg_id) DO UPDATE SET
                    content           = excluded.content,
                    edited_timestamp  = excluded.edited_timestamp,
                    guild_id          = COALESCE(excluded.guild_id,     messages.guild_id),
                    guild_name        = COALESCE(excluded.guild_name,   messages.guild_name),
                    channel_name      = COALESCE(excluded.channel_name, messages.channel_name)
                 WHERE excluded.edited_timestamp IS NOT NULL
                   AND (messages.edited_timestamp IS NULL
                        OR excluded.edited_timestamp > messages.edited_timestamp)",
            )?;
            let mut att_stmt = tx.prepare(
                "INSERT OR REPLACE INTO attachments
                    (msg_id, attach_id, filename, url, content_type, size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for m in msgs {
                msg_stmt.execute(params![
                    m.msg_id,
                    m.channel_id,
                    m.sender_id,
                    m.sender_name,
                    m.content,
                    m.timestamp.to_rfc3339(),
                    m.guild_id,
                    m.guild_name,
                    m.channel_name,
                    m.edited_timestamp,
                ])?;
                for a in &m.attachments {
                    let size_i64 = i64::try_from(a.size)
                        .with_context(|| format!("attachment size {} overflows i64", a.size))?;
                    att_stmt.execute(params![
                        m.msg_id,
                        a.attach_id,
                        a.filename,
                        a.url,
                        a.content_type,
                        size_i64,
                    ])?;
                }
            }
        }
        tx.commit()?;
        let post_total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        let delta = (post_total - pre_total).max(0);
        Ok(usize::try_from(delta).unwrap_or(usize::MAX))
    }

    /// Apply a `MESSAGE_UPDATE` from the gateway to the local archive.
    /// No-op if the message is unknown (we only track edits to messages we
    /// have already synced). Returns true iff a row was updated.
    pub fn apply_edit(
        &self,
        msg_id: &str,
        new_content: Option<&str>,
        edited_at: Option<&str>,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages
                SET content = COALESCE(?2, content),
                    edited_timestamp = COALESCE(?3, edited_timestamp)
                WHERE msg_id = ?1",
            params![msg_id, new_content, edited_at],
        )?;
        Ok(n > 0)
    }

    /// Mark a message as deleted by appending `[deleted]` to its content.
    /// Discord doesn't tell us *when* a delete happened, only that it did;
    /// keeping the row preserves the archive while making the state visible.
    pub fn apply_delete(&self, msg_id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE messages
                SET content = CASE
                    WHEN content LIKE '%[deleted]%' THEN content
                    ELSE COALESCE(content,'') || ' [deleted]'
                END
                WHERE msg_id = ?1",
            params![msg_id],
        )?;
        Ok(n > 0)
    }

    pub fn search(
        &self,
        keyword: &str,
        channel_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<StoredMessage>> {
        // Prefer FTS5 when available; fall back to LIKE for legacy DBs or
        // SQLite builds compiled without FTS5.
        let rows: Vec<StoredMessage> = if self.has_fts()? {
            let mut sql = String::from(
                "SELECT m.msg_id, m.channel_id, m.sender_id, m.sender_name, m.content, m.timestamp,
                        m.guild_id, m.guild_name, m.channel_name, m.edited_timestamp
                 FROM messages_fts f
                 JOIN messages m ON m.rowid = f.rowid
                 WHERE f.content MATCH ?1",
            );
            if channel_id.is_some() {
                sql.push_str(" AND m.channel_id = ?2");
                sql.push_str(" ORDER BY CAST(m.msg_id AS INTEGER) DESC LIMIT ?3");
            } else {
                sql.push_str(" ORDER BY CAST(m.msg_id AS INTEGER) DESC LIMIT ?2");
            }
            // Quote the keyword to treat it as a phrase — keeps user-typed
            // punctuation from being interpreted as FTS5 operators.
            let phrase = format!("\"{}\"", keyword.replace('"', "\"\""));
            let mut stmt = self.conn.prepare(&sql)?;
            if let Some(cid) = channel_id {
                stmt.query_map(params![phrase, cid, limit], row_to_msg)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map(params![phrase, limit], row_to_msg)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        } else {
            let pattern = format!("%{}%", keyword.to_lowercase());
            let mut sql = String::from(
                "SELECT msg_id, channel_id, sender_id, sender_name, content, timestamp,
                        guild_id, guild_name, channel_name, edited_timestamp
                 FROM messages
                 WHERE LOWER(content) LIKE ?1",
            );
            if channel_id.is_some() {
                sql.push_str(" AND channel_id = ?2");
                sql.push_str(" ORDER BY CAST(msg_id AS INTEGER) DESC LIMIT ?3");
            } else {
                sql.push_str(" ORDER BY CAST(msg_id AS INTEGER) DESC LIMIT ?2");
            }
            let mut stmt = self.conn.prepare(&sql)?;
            if let Some(cid) = channel_id {
                stmt.query_map(params![pattern, cid, limit], row_to_msg)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map(params![pattern, limit], row_to_msg)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        let mut out = rows;
        out.reverse(); // newest at bottom
        Ok(out)
    }

    /// Stored attachment URLs for a channel, optionally restricted to the
    /// last `hours` window. Used by `dc download`. Returned as
    /// `(msg_id, attach_id, filename, url, content_type, size)` so callers
    /// can dedupe and report.
    pub fn attachments_for_channel(
        &self,
        channel_id: &str,
        hours: Option<i64>,
    ) -> Result<Vec<AttachmentRow>> {
        let cutoff =
            hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());
        let mut sql = String::from(
            "SELECT a.msg_id, a.attach_id, COALESCE(a.filename,'file'),
                    COALESCE(a.url,''), a.content_type, COALESCE(a.size, 0)
             FROM attachments a
             JOIN messages m ON m.msg_id = a.msg_id
             WHERE m.channel_id = ?1",
        );
        if cutoff.is_some() {
            sql.push_str(" AND m.timestamp >= ?2");
        }
        sql.push_str(" ORDER BY m.timestamp ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<AttachmentRow> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        };
        let rows: Vec<_> = match cutoff {
            Some(c) => stmt
                .query_map(params![channel_id, c], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            None => stmt
                .query_map(params![channel_id], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    pub fn recent(
        &self,
        channel_id: Option<&str>,
        hours: Option<i64>,
        limit: i64,
    ) -> Result<Vec<StoredMessage>> {
        let mut sql = String::from(
            "SELECT msg_id, channel_id, sender_id, sender_name, content, timestamp,
                    guild_id, guild_name, channel_name, edited_timestamp
             FROM messages WHERE 1=1",
        );
        let mut bind_idx = 1usize;
        if channel_id.is_some() {
            sql.push_str(&format!(" AND channel_id = ?{}", bind_idx));
            bind_idx += 1;
        }
        if hours.is_some() {
            sql.push_str(&format!(" AND timestamp >= ?{}", bind_idx));
            bind_idx += 1;
        }
        sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT ?{}", bind_idx));

        let cutoff = hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());

        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<StoredMessage> = match (channel_id, cutoff.as_deref()) {
            (Some(cid), Some(c)) => stmt
                .query_map(params![cid, c, limit], row_to_msg)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (Some(cid), None) => stmt
                .query_map(params![cid, limit], row_to_msg)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (None, Some(c)) => stmt
                .query_map(params![c, limit], row_to_msg)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (None, None) => stmt
                .query_map(params![limit], row_to_msg)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        let mut out = rows;
        out.reverse();
        Ok(out)
    }

    pub fn stats(&self) -> Result<Vec<(String, Option<String>, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, MAX(channel_name) AS channel_name, COUNT(*) as cnt
             FROM messages
             GROUP BY channel_id
             ORDER BY cnt DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Resolve a channel name to a single channel_id from the local archive.
    /// Distinguishes 0-match (with hint to run `dc sync-all`) from many-match.
    pub fn resolve_channel_name(&self, name: &str) -> Result<String> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT channel_id FROM messages WHERE LOWER(channel_name) = LOWER(?1)",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![name], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match ids.as_slice() {
            [] => {
                // Check whether the local DB has any rows at all to give a better hint.
                let total: i64 = self
                    .conn
                    .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
                    .unwrap_or(0);
                if total == 0 {
                    Err(anyhow!(
                        "No messages indexed yet — run `discord dc sync-all` first."
                    ))
                } else {
                    Err(anyhow!(
                        "No channel matches '{}' in the local archive.",
                        name
                    ))
                }
            }
            [only] => Ok(only.clone()),
            many => Err(anyhow!(
                "{} channels match '{}'. Use a channel ID instead.",
                many.len(),
                name
            )),
        }
    }

    pub fn today(&self, channel_id: Option<&str>) -> Result<Vec<StoredMessage>> {
        let midnight = Local::now()
            .date_naive()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
            .and_local_timezone(Local)
            .earliest()
            .unwrap_or_else(|| Utc::now().with_timezone(&Local))
            .with_timezone(&Utc)
            .to_rfc3339();

        let mut sql = String::from(
            "SELECT msg_id, channel_id, sender_id, sender_name, content, timestamp,
                    guild_id, guild_name, channel_name, edited_timestamp
             FROM messages WHERE timestamp >= ?1",
        );
        if channel_id.is_some() {
            sql.push_str(" AND channel_id = ?2");
        }
        sql.push_str(" ORDER BY channel_name, timestamp ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<StoredMessage> = if let Some(cid) = channel_id {
            stmt.query_map(params![midnight, cid], row_to_msg)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![midnight], row_to_msg)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    }

    pub fn top_senders(
        &self,
        channel_id: Option<&str>,
        hours: Option<i64>,
        limit: i64,
    ) -> Result<Vec<(String, i64, String, String)>> {
        let mut sql = String::from(
            "SELECT COALESCE(MAX(sender_name), 'Unknown') as sender_name,
                    COUNT(*) as msg_count,
                    MIN(timestamp) as first_msg,
                    MAX(timestamp) as last_msg
             FROM messages WHERE 1=1",
        );
        let mut bind_idx = 1usize;
        if channel_id.is_some() {
            sql.push_str(&format!(" AND channel_id = ?{}", bind_idx));
            bind_idx += 1;
        }
        if hours.is_some() {
            sql.push_str(&format!(" AND timestamp >= ?{}", bind_idx));
            bind_idx += 1;
        }
        sql.push_str(" GROUP BY COALESCE(sender_id, sender_name)");
        sql.push_str(&format!(" ORDER BY msg_count DESC LIMIT ?{}", bind_idx));

        let cutoff = hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());
        let mut stmt = self.conn.prepare(&sql)?;

        let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(String, i64, String, String)> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        };

        let rows: Vec<(String, i64, String, String)> = match (channel_id, cutoff.as_deref()) {
            (Some(cid), Some(c)) => stmt
                .query_map(params![cid, c, limit], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (Some(cid), None) => stmt
                .query_map(params![cid, limit], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (None, Some(c)) => stmt
                .query_map(params![c, limit], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (None, None) => stmt
                .query_map(params![limit], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    pub fn timeline(
        &self,
        channel_id: Option<&str>,
        hours: Option<i64>,
        by: &str,
    ) -> Result<Vec<(String, i64)>> {
        let bucket_expr = if by == "hour" {
            "substr(timestamp, 1, 13)"
        } else {
            "substr(timestamp, 1, 10)"
        };

        let mut sql = format!(
            "SELECT {} as bucket, COUNT(*) as cnt FROM messages WHERE 1=1",
            bucket_expr
        );
        let mut bind_idx = 1usize;
        if channel_id.is_some() {
            sql.push_str(&format!(" AND channel_id = ?{}", bind_idx));
            bind_idx += 1;
        }
        if hours.is_some() {
            sql.push_str(&format!(" AND timestamp >= ?{}", bind_idx));
        }
        sql.push_str(" GROUP BY 1 ORDER BY 1");

        let cutoff = hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());
        let mut stmt = self.conn.prepare(&sql)?;

        let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(String, i64)> {
            Ok((r.get(0)?, r.get(1)?))
        };

        let rows: Vec<(String, i64)> = match (channel_id, cutoff.as_deref()) {
            (Some(cid), Some(c)) => stmt
                .query_map(params![cid, c], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (Some(cid), None) => stmt
                .query_map(params![cid], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (None, Some(c)) => stmt
                .query_map(params![c], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            (None, None) => stmt
                .query_map([], mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    pub fn count_channel(&self, channel_id: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            params![channel_id],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    pub fn purge(&self, channel_id: &str) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM messages WHERE channel_id = ?1",
            params![channel_id],
        )?;
        Ok(deleted)
    }

    /// `dc history` resume cursor: oldest msg_id we've fetched per channel.
    pub fn history_cursor(&self, channel_id: &str) -> Result<Option<String>> {
        let key = format!("history_cursor:{}", channel_id);
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(row)
    }

    pub fn set_history_cursor(&self, channel_id: &str, msg_id: &str) -> Result<()> {
        let key = format!("history_cursor:{}", channel_id);
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, msg_id],
        )?;
        Ok(())
    }
}

fn row_to_msg(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    let ts_str: String = r.get(5)?;
    // A bad timestamp on a stored row is a real corruption signal; surface
    // it instead of silently substituting `Utc::now()` (which would corrupt
    // every chronological query).
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?;
    Ok(StoredMessage {
        msg_id: r.get(0)?,
        channel_id: r.get(1)?,
        sender_id: r.get(2)?,
        sender_name: r.get(3)?,
        content: r.get(4)?,
        timestamp,
        guild_id: r.get(6)?,
        guild_name: r.get(7)?,
        channel_name: r.get(8)?,
        edited_timestamp: r.get::<_, Option<String>>(9)?,
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(
        id: &str,
        channel: &str,
        sender_id: &str,
        sender: &str,
        content: &str,
        channel_name: &str,
    ) -> StoredMessage {
        StoredMessage {
            msg_id: id.into(),
            channel_id: channel.into(),
            sender_id: Some(sender_id.into()),
            sender_name: sender.into(),
            content: content.into(),
            timestamp: Utc::now(),
            guild_id: Some("g1".into()),
            guild_name: Some("Test".into()),
            channel_name: Some(channel_name.into()),
            edited_timestamp: None,
            attachments: Vec::new(),
        }
    }

    fn sample_messages() -> Vec<StoredMessage> {
        vec![
            make_msg("100", "c1", "u1", "alice", "Hello world from rust", "general"),
            make_msg("101", "c1", "u2", "bob", "rust is fast", "general"),
            make_msg("200", "c2", "u3", "carol", "another message", "random"),
        ]
    }

    #[test]
    fn insert_and_query() {
        let mut db = Db::open_in_memory().unwrap();
        let msgs = sample_messages();
        let inserted = db.insert_batch(&msgs).unwrap();
        assert_eq!(inserted, 3);

        // Re-insert is idempotent (ON CONFLICT WHERE clause filters edits with no edited_timestamp).
        let inserted2 = db.insert_batch(&msgs).unwrap();
        assert_eq!(inserted2, 0);

        // search
        let r = db.search("rust", None, 50).unwrap();
        assert_eq!(r.len(), 2);

        let r = db.search("rust", Some("c1"), 50).unwrap();
        assert_eq!(r.len(), 2);

        let r = db.search("zebra", None, 50).unwrap();
        assert_eq!(r.len(), 0);

        // recent
        let r = db.recent(None, None, 100).unwrap();
        assert_eq!(r.len(), 3);

        // stats
        let s = db.stats().unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].2, 2); // c1 has 2 messages

        // last_msg_id picks the numerically-max snowflake per channel
        assert_eq!(db.last_msg_id("c1").unwrap(), Some("101".to_string()));
        assert_eq!(db.last_msg_id("c2").unwrap(), Some("200".to_string()));
        assert_eq!(db.last_msg_id("nope").unwrap(), None);
    }

    #[test]
    fn last_msg_ids_matches_per_channel() {
        // Realistic 19-digit snowflakes near u64 range — exercises the
        // batched-vs-single semantic equivalence the reviewer flagged.
        let mut db = Db::open_in_memory().unwrap();
        let msgs = vec![
            make_msg(
                "1234567890123456789",
                "c1",
                "u1",
                "alice",
                "a",
                "general",
            ),
            make_msg(
                "1234567890123456790",
                "c1",
                "u1",
                "alice",
                "b",
                "general",
            ),
            make_msg(
                "9000000000000000001",
                "c2",
                "u2",
                "bob",
                "c",
                "random",
            ),
        ];
        db.insert_batch(&msgs).unwrap();

        let single_c1 = db.last_msg_id("c1").unwrap();
        let single_c2 = db.last_msg_id("c2").unwrap();
        let single_nope = db.last_msg_id("nope").unwrap();

        let batched = db.last_msg_ids(&["c1", "c2", "nope"]).unwrap();

        assert_eq!(batched.get("c1").cloned(), single_c1);
        assert_eq!(batched.get("c2").cloned(), single_c2);
        assert!(!batched.contains_key("nope"));
        assert!(single_nope.is_none());
    }

    #[test]
    fn resolve_channel_name_empty_db_hint() {
        let db = Db::open_in_memory().unwrap();
        let err = db.resolve_channel_name("general").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("dc sync-all"),
            "expected sync-all hint, got: {}",
            msg
        );
    }

    #[test]
    fn resolve_channel_name_unique() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_batch(&sample_messages()).unwrap();
        assert_eq!(db.resolve_channel_name("general").unwrap(), "c1");
        assert_eq!(db.resolve_channel_name("random").unwrap(), "c2");
    }

    #[test]
    fn resolve_channel_name_missing() {
        let mut db = Db::open_in_memory().unwrap();
        db.insert_batch(&sample_messages()).unwrap();
        let err = db.resolve_channel_name("does-not-exist").unwrap_err();
        assert!(err.to_string().contains("No channel matches"));
    }
}
