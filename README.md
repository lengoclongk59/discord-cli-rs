# discord-cli (Rust)

> [!WARNING]
> **Deprecated — this repository is archived.** The CLI now lives in
> [`discord-user-rs`](https://github.com/lengoclongk59/discord-user-rs) as the `discord` binary
> (`cargo install discord-user-rs --features cli`). That version is a superset: archive/sync/search
> plus send, edit, react, threads, roles, bans, invites, webhooks, and more.

Local-first Rust port of [jackwener/discord-cli](https://github.com/jackwener/discord-cli). Read-only Discord archival CLI built on top of the local [`discord-user-rs`](../discord-user-rs) crate.

## Warning

`discord-cli` reads a Discord **user token** from your local Discord/browser session. Discord may restrict or suspend accounts that automate user-token traffic. Use only on accounts you control.

## Features (MVP)

- Auto-discover the user token from the local Discord install on Windows (DPAPI + LevelDB scan)
- Sync messages from accessible text channels into a local SQLite database
- Search, list recent, and view per-channel statistics offline
- Rich text output (no `--json`/`--yaml` in the MVP)

## Install

This crate lives next to `discord-user-rs` as a sibling directory:

```
C:\RustProjects\
├── discord-user-rs\
└── discord-cli\          <-- this crate
```

```powershell
cd C:\RustProjects\discord-cli
cargo build --release
```

The binary is produced at `target\release\discord.exe`.

## Quick start

```powershell
# Auto-discover and save the Discord token (Windows; close Discord first)
.\target\release\discord.exe auth --save

# Verify
.\target\release\discord.exe status
.\target\release\discord.exe whoami

# List joined guilds and their text channels
.\target\release\discord.exe dc guilds
.\target\release\discord.exe dc channels <GUILD_ID_OR_NAME>

# Bootstrap local archive (default 200 messages per channel)
.\target\release\discord.exe dc sync-all -n 200

# Query the local archive
.\target\release\discord.exe search rust
.\target\release\discord.exe recent -n 50
.\target\release\discord.exe stats
```

## Commands

| Command | Description |
|---------|-------------|
| `auth --save` | Auto-discover and save the Discord token (Windows-only) |
| `status` | Verify the configured token works (exits 1 on failure) |
| `whoami` | Print profile of the authenticated user (exits 1 on failure) |
| `dc guilds` | List joined guilds |
| `dc channels <GUILD>` | List text channels in a guild (id or name) |
| `dc sync <CHANNEL> [-n N]` | Incremental sync: fetch new messages after the last stored id |
| `dc sync-all [-n N]` | Discover all guilds + text channels and sync each |
| `search <KEYWORD> [-c CH] [-n N]` | Search local archive for keyword |
| `recent [-c CH] [--hours H] [-n N]` | Show newest stored messages |
| `stats` | Per-channel message counts |

Global flags:
- `--token <T>` — override token resolution
- `--db <PATH>` — override the SQLite location (default `./discord.db`)
- `--no-color` — disable ANSI color (also honored via `NO_COLOR` env)

## Token resolution

Order:
1. `--token` flag
2. `DISCORD_TOKEN` env var
3. `./.env` (`DISCORD_TOKEN=...`)

`auth --save` writes/upserts `DISCORD_TOKEN=<token>` into `./.env` in the current working directory.

## Architecture

- Depends on `discord-user-rs` via `path = "../discord-user-rs", default-features = false`.
- Uses `discord_user::client::DiscordHttpClient` directly (NOT `DiscordUser`); never starts the gateway.
- Wraps `DiscordHttpClient` in a `ReadOnlyHttp` newtype that exposes only `.get()` — the read-only invariant is enforced at the type level.
- A `tests/readonly.rs` test greps `src/` for any `.post(/.patch(/.put(/.delete(` calls and fails the suite if found (excluding the single sanctioned exception in `src/auth/windows.rs::validate_token`).

## Non-goals (MVP)

`tail`, `export`, `purge`, `today`, `top`, `timeline`, `dc members`, `dc info`, `--json`/`--yaml`, macOS/Linux token discovery, browser token discovery, gateway-driven live archival.

## License

MIT
