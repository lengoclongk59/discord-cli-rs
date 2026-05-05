//! Shared helpers for command implementations.

use anyhow::Result;

use crate::db::Db;

pub fn resolve_channel(db: &Db, input: Option<&str>) -> Result<Option<String>> {
    match input {
        Some(c) if c.chars().all(|ch| ch.is_ascii_digit()) => Ok(Some(c.to_string())),
        Some(name) => Ok(Some(db.resolve_channel_name(name)?)),
        None => Ok(None),
    }
}

pub fn resolve_channel_required(db: &Db, input: &str) -> Result<String> {
    if input.chars().all(|c| c.is_ascii_digit()) {
        Ok(input.to_string())
    } else {
        db.resolve_channel_name(input)
    }
}
