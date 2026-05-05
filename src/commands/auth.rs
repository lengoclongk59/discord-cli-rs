//! `discord auth [--save]` — auto-discover the user token.

use anyhow::Result;

use crate::auth;
use crate::cli::AuthArgs;
use crate::config;
use crate::output;

pub async fn run(args: AuthArgs) -> Result<()> {
    output::dim(
        "Warning: discord-cli reads a Discord user token from your local Discord/browser session. \
         This may violate Discord's ToS. Use only on accounts you control.",
    );
    output::dim("Scanning for Discord tokens...");

    match auth::find_and_save_token().await {
        Ok(found) => {
            output::success(&format!(
                "Valid token from {}: {} (logged in as {})",
                found.source,
                config::mask_token(&found.token),
                found.username
            ));
            if args.save {
                let path = config::write_token_to_env(&found.token)?;
                output::success(&format!("Saved to {}", path.display()));
            } else {
                output::dim("\nRun with --save to write the token to .env");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}
