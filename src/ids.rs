//! Discord snowflake newtypes.
//!
//! Discord IDs are 64-bit snowflakes serialized on the wire as decimal
//! strings. The compiler can't tell `guild_id` from `channel_id` when both
//! are `String`; wrapping each kind in a distinct newtype makes argument
//! swaps a compile error.
//!
//! Storage layer (SQLite) keeps `TEXT` columns to avoid a schema migration —
//! `Display` round-trips identically to the wire format, so the newtype
//! converts at the API/DB boundary.

use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! snowflake {
    ($Name:ident, $kind:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[allow(dead_code)]
        pub struct $Name(pub u64);

        impl $Name {
            #[allow(dead_code)]
            pub fn as_u64(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $Name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $Name {
            type Err = anyhow::Error;
            fn from_str(s: &str) -> Result<Self> {
                if s.is_empty() {
                    return Err(anyhow!("empty {} snowflake", $kind));
                }
                let n: u64 = s
                    .parse()
                    .with_context(|| format!("invalid {} snowflake: {:?}", $kind, s))?;
                Ok($Name(n))
            }
        }

        impl Serialize for $Name {
            fn serialize<S: Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
                ser.collect_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $Name {
            fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
                let s = String::deserialize(de)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

snowflake!(GuildId, "guild");
snowflake!(ChannelId, "channel");
snowflake!(UserId, "user");
snowflake!(MessageId, "message");
snowflake!(RoleId, "role");
snowflake!(EmojiId, "emoji");
snowflake!(StickerId, "sticker");
snowflake!(ThreadId, "thread");
snowflake!(AttachmentId, "attachment");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid() {
        let g: GuildId = "123456789012345678".parse().unwrap();
        assert_eq!(g.as_u64(), 123456789012345678u64);
        assert_eq!(g.to_string(), "123456789012345678");
    }

    #[test]
    fn rejects_garbage() {
        assert!("not-a-snowflake".parse::<GuildId>().is_err());
        assert!("".parse::<ChannelId>().is_err());
    }

    #[test]
    fn type_isolation() {
        let g: GuildId = "1".parse().unwrap();
        let c: ChannelId = "1".parse().unwrap();
        // The compiler enforces these are distinct types.
        // Uncommenting the next line would not compile:
        // let _: GuildId = c;
        let _ = (g, c);
    }
}
