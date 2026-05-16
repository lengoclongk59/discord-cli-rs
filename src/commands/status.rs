//! `discord status` — verify the configured token is valid.

use anyhow::{anyhow, Result};

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;

pub async fn run(ctx: &Ctx) -> Result<()> {
    let token = match config::resolve_token(ctx.token_flag.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            if ctx.json {
                output::print_json(&serde_json::json!({"ok": false, "error": format!("{}", e)}));
            }
            return Err(e);
        }
    };
    let api = Api::new(&token);
    match api.get_me().await {
        Ok(me) => {
            if ctx.json {
                output::print_json(&serde_json::json!({
                    "ok": true,
                    "id": me.id,
                    "username": me.username,
                    "global_name": me.global_name,
                }));
                return Ok(());
            }
            let display = me.global_name.as_deref().unwrap_or(&me.username);
            output::success(&format!("Authenticated as {} (@{})", display, me.username));
            Ok(())
        }
        Err(e) => {
            if ctx.json {
                output::print_json(&serde_json::json!({"ok": false, "error": format!("{}", e)}));
            }
            // Returning Err lets `main` set exit code 1 while running
            // Drop on token strings (zeroize, runtime shutdown).
            Err(anyhow!("Token check failed: {}", e))
        }
    }
}
