//! SQLite storage for the Discord message archive.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, NaiveTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::types::StoredMessage;

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
    /// SQLite catalog lookups are cheap.
    fn has_fts(&self) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
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
                    att_stmt.execute(params![
                        m.msg_id,
                        a.attach_id,
                        a.filename,
                        a.url,
                        a.content_type,
                        a.size as i64,
                    ])?;
                }
            }
        }
        tx.commit()?;
        let post_total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        Ok((post_total - pre_total).max(0) as usize)
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
        let rows: Vec<StoredMessage> = if self.has_fts() {
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
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                stmt.query_map(params![phrase, limit], row_to_msg)?
                    .filter_map(|r| r.ok())
                    .collect()
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
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                stmt.query_map(params![pattern, limit], row_to_msg)?
                    .filter_map(|r| r.ok())
                    .collect()
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
    ) -> Result<Vec<(String, String, String, String, Option<String>, i64)>> {
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
        let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
        )> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        };
        let rows: Vec<_> = match cutoff {
            Some(c) => stmt
                .query_map(params![channel_id, c], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            None => stmt
                .query_map(params![channel_id], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
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
        let mut cid_pos: Option<usize> = None;
        let mut hours_pos: Option<usize> = None;
        if channel_id.is_some() {
            sql.push_str(&format!(" AND channel_id = ?{}", bind_idx));
            cid_pos = Some(bind_idx);
            bind_idx += 1;
        }
        if hours.is_some() {
            sql.push_str(&format!(" AND timestamp >= ?{}", bind_idx));
            hours_pos = Some(bind_idx);
            bind_idx += 1;
        }
        sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT ?{}", bind_idx));

        let cutoff = hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());

        let mut stmt = self.conn.prepare(&sql)?;
        let rows: Vec<StoredMessage> = match (cid_pos, hours_pos) {
            (Some(_), Some(_)) => stmt
                .query_map(
                    params![channel_id.unwrap(), cutoff.unwrap(), limit],
                    row_to_msg,
                )?
                .filter_map(|r| r.ok())
                .collect(),
            (Some(_), None) => stmt
                .query_map(params![channel_id.unwrap(), limit], row_to_msg)?
                .filter_map(|r| r.ok())
                .collect(),
            (None, Some(_)) => stmt
                .query_map(params![cutoff.unwrap(), limit], row_to_msg)?
                .filter_map(|r| r.ok())
                .collect(),
            (None, None) => stmt
                .query_map(params![limit], row_to_msg)?
                .filter_map(|r| r.ok())
                .collect(),
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
            .filter_map(|r| r.ok())
            .collect();
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
            .filter_map(|r| r.ok())
            .collect();
        match ids.len() {
            0 => {
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
            1 => Ok(ids.into_iter().next().unwrap()),
            n => Err(anyhow!(
                "{} channels match '{}'. Use a channel ID instead.",
                n,
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
                .filter_map(|r| r.ok())
                .collect()
        } else {
            stmt.query_map(params![midnight], row_to_msg)?
                .filter_map(|r| r.ok())
                .collect()
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
        let mut cid_pos: Option<usize> = None;
        let mut hours_pos: Option<usize> = None;

        if channel_id.is_some() {
            sql.push_str(&format!(" AND channel_id = ?{}", bind_idx));
            cid_pos = Some(bind_idx);
            bind_idx += 1;
        }
        if hours.is_some() {
            sql.push_str(&format!(" AND timestamp >= ?{}", bind_idx));
            hours_pos = Some(bind_idx);
            bind_idx += 1;
        }
        sql.push_str(" GROUP BY COALESCE(sender_id, sender_name)");
        sql.push_str(&format!(" ORDER BY msg_count DESC LIMIT ?{}", bind_idx));

        let cutoff = hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());
        let mut stmt = self.conn.prepare(&sql)?;

        let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(String, i64, String, String)> {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        };

        let rows: Vec<(String, i64, String, String)> = match (cid_pos, hours_pos) {
            (Some(_), Some(_)) => stmt
                .query_map(params![channel_id.unwrap(), cutoff.unwrap(), limit], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            (Some(_), None) => stmt
                .query_map(params![channel_id.unwrap(), limit], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            (None, Some(_)) => stmt
                .query_map(params![cutoff.unwrap(), limit], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            (None, None) => stmt
                .query_map(params![limit], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
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
        let mut cid_pos: Option<usize> = None;
        let mut hours_pos: Option<usize> = None;

        if channel_id.is_some() {
            sql.push_str(&format!(" AND channel_id = ?{}", bind_idx));
            cid_pos = Some(bind_idx);
            bind_idx += 1;
        }
        if hours.is_some() {
            sql.push_str(&format!(" AND timestamp >= ?{}", bind_idx));
            hours_pos = Some(bind_idx);
        }
        sql.push_str(" GROUP BY 1 ORDER BY 1");

        let cutoff = hours.map(|h| (Utc::now() - chrono::Duration::hours(h)).to_rfc3339());
        let mut stmt = self.conn.prepare(&sql)?;

        let mapper = |r: &rusqlite::Row<'_>| -> rusqlite::Result<(String, i64)> {
            Ok((r.get(0)?, r.get(1)?))
        };

        let rows: Vec<(String, i64)> = match (cid_pos, hours_pos) {
            (Some(_), Some(_)) => stmt
                .query_map(params![channel_id.unwrap(), cutoff.unwrap()], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            (Some(_), None) => stmt
                .query_map(params![channel_id.unwrap()], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            (None, Some(_)) => stmt
                .query_map(params![cutoff.unwrap()], mapper)?
                .filter_map(|r| r.ok())
                .collect(),
            (None, None) => stmt.query_map([], mapper)?.filter_map(|r| r.ok()).collect(),
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
        edited_timestamp: r.get(9).ok(),
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
