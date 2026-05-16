//! Typed enums for bounded Discord integer wire values.
//!
//! Discord sends these as raw ints; we map them to enums so the compiler
//! checks exhaustiveness when new variants are added. `Unknown(u32)`
//! preserves wire round-trip for values Discord adds later — display logic
//! renders them as `[unknown: N]` rather than `?`.

use std::fmt;

macro_rules! wire_enum {
    (
        $Name:ident($Wire:ty) {
            $( $variant:ident = $value:expr => $label:expr ),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[allow(dead_code)]
        pub enum $Name {
            $( $variant, )*
            Unknown($Wire),
        }

        impl From<$Wire> for $Name {
            fn from(v: $Wire) -> Self {
                match v {
                    $( $value => $Name::$variant, )*
                    other => $Name::Unknown(other),
                }
            }
        }

        impl fmt::Display for $Name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $( $Name::$variant => f.write_str($label), )*
                    $Name::Unknown(n) => write!(f, "[unknown: {}]", n),
                }
            }
        }
    };
}

wire_enum!(PremiumType(u32) {
    None         = 0 => "None",
    NitroClassic = 1 => "Nitro Classic",
    Nitro        = 2 => "Nitro",
    NitroBasic   = 3 => "Nitro Basic",
});

wire_enum!(ScheduledEventStatus(u32) {
    Scheduled = 1 => "Scheduled",
    Active    = 2 => "Active",
    Completed = 3 => "Completed",
    Cancelled = 4 => "Cancelled",
});

wire_enum!(RelationshipType(u8) {
    Friend     = 1 => "Friend",
    Blocked    = 2 => "Blocked",
    PendingIn  = 3 => "Pending In",
    PendingOut = 4 => "Pending Out",
    Implicit   = 5 => "Implicit",
});

wire_enum!(StickerFormat(u32) {
    Png    = 1 => "PNG",
    Apng   = 2 => "APNG",
    Lottie = 3 => "Lottie",
    Gif    = 4 => "GIF",
});

/// Audit-log action groupings (mirrors Discord's documented ranges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AuditAction {
    GuildUpdate,
    ChannelOp,
    OverwriteOp,
    MemberOp,
    RoleOp,
    InviteOp,
    WebhookOp,
    EmojiOp,
    MessageOp,
    IntegrationOp,
    StickerOp,
    EventOp,
    ThreadOp,
    AutoModOp,
    Unknown(u32),
}

impl From<u32> for AuditAction {
    fn from(v: u32) -> Self {
        match v {
            1 => Self::GuildUpdate,
            10..=12 => Self::ChannelOp,
            13..=15 => Self::OverwriteOp,
            20..=28 => Self::MemberOp,
            30..=32 => Self::RoleOp,
            40..=42 => Self::InviteOp,
            50..=52 => Self::WebhookOp,
            60..=62 => Self::EmojiOp,
            72..=75 => Self::MessageOp,
            80..=84 => Self::IntegrationOp,
            90..=92 => Self::StickerOp,
            100..=102 => Self::EventOp,
            110..=112 => Self::ThreadOp,
            140..=145 => Self::AutoModOp,
            other => Self::Unknown(other),
        }
    }
}

impl fmt::Display for AuditAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::GuildUpdate => "Guild Update",
            Self::ChannelOp => "Channel Op",
            Self::OverwriteOp => "Overwrite Op",
            Self::MemberOp => "Member Op",
            Self::RoleOp => "Role Op",
            Self::InviteOp => "Invite Op",
            Self::WebhookOp => "Webhook Op",
            Self::EmojiOp => "Emoji Op",
            Self::MessageOp => "Message Op",
            Self::IntegrationOp => "Integration Op",
            Self::StickerOp => "Sticker Op",
            Self::EventOp => "Event Op",
            Self::ThreadOp => "Thread Op",
            Self::AutoModOp => "AutoMod Op",
            Self::Unknown(n) => return write!(f, "[unknown: {}]", n),
        };
        f.write_str(s)
    }
}

/// `dc tail` mode: snapshot-and-exit vs live Gateway stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailMode {
    Snapshot,
    Stream,
}

impl TailMode {
    pub fn from_once_flag(once: bool) -> Self {
        if once {
            Self::Snapshot
        } else {
            Self::Stream
        }
    }
}

/// Thread lifecycle as encoded by Discord's two booleans
/// (`archived`, `locked`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ThreadState {
    Open,
    Archived,
    Locked,
    ArchivedAndLocked,
}

impl ThreadState {
    pub fn from_flags(archived: bool, locked: bool) -> Self {
        match (archived, locked) {
            (false, false) => Self::Open,
            (true, false) => Self::Archived,
            (false, true) => Self::Locked,
            (true, true) => Self::ArchivedAndLocked,
        }
    }

    #[allow(dead_code)]
    pub fn is_archived(self) -> bool {
        matches!(self, Self::Archived | Self::ArchivedAndLocked)
    }
}

impl fmt::Display for ThreadState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Open => "open",
            Self::Archived => "archived",
            Self::Locked => "locked",
            Self::ArchivedAndLocked => "archived+locked",
        };
        f.write_str(s)
    }
}
