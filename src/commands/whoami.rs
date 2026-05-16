//! `discord whoami` — print the current user profile.

use anyhow::{anyhow, Result};

use crate::api::Api;
use crate::commands::Ctx;
use crate::config;
use crate::output;
use crate::wire_enums::PremiumType;

pub async fn run(ctx: &Ctx) -> Result<()> {
    let token = config::resolve_token(ctx.token_flag.as_deref())?;
    let api = Api::new(&token);
    let me = match api.get_me().await {
        Ok(m) => m,
        Err(e) => {
            if ctx.json {
                output::print_json(&serde_json::json!({"ok": false, "error": format!("{}", e)}));
            }
            return Err(anyhow!("whoami failed: {}", e));
        }
    };

    if ctx.json {
        output::print_json(&me);
        return Ok(());
    }

    let premium = PremiumType::from(me.premium_type).to_string();
    let rows = vec![
        vec!["id".into(), me.id.clone()],
        vec!["username".into(), format!("@{}", me.username)],
        vec![
            "global_name".into(),
            me.global_name.clone().unwrap_or_default(),
        ],
        vec!["email".into(), me.email.unwrap_or_default()],
        vec!["phone".into(), me.phone.unwrap_or_default()],
        vec![
            "mfa".into(),
            if me.mfa_enabled { "✓" } else { "✗" }.to_string(),
        ],
        vec!["nitro".into(), premium],
    ];
    output::print_table(&["field", "value"], &rows);
    Ok(())
}
