//! Console output helpers: rich tables (comfy-table) and colored status lines.

use colored::Colorize;
use comfy_table::{presets::UTF8_BORDERS_ONLY, Cell, Table};
use serde::Serialize;

use crate::types::StoredMessage;

pub fn print_json<T: Serialize>(data: &T) {
    match serde_json::to_string_pretty(data) {
        Ok(s) => println!("{}", s),
        Err(e) => err(&format!("JSON serialization failed: {}", e)),
    }
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut t = Table::new();
    t.load_preset(UTF8_BORDERS_ONLY);
    t.set_header(headers.iter().map(Cell::new));
    for row in rows {
        t.add_row(row.iter().map(Cell::new));
    }
    println!("{}", t);
}

pub fn success(msg: &str) {
    // Final-result lines stay on stdout — this is the value the user came
    // for and downstream pipes (jq, grep) may want it.
    println!("{} {}", "✓".green().bold(), msg);
}

pub fn err(msg: &str) {
    eprintln!("{} {}", "✗".red().bold(), msg.red());
}

pub fn warn(msg: &str) {
    eprintln!("{} {}", "!".yellow().bold(), msg.yellow());
}

/// Progress / status / dim chrome. Goes to stderr so it doesn't pollute
/// piped JSON or text output. (Older versions of this CLI wrote to stdout
/// and broke `discord dc sync ... | jq`.)
pub fn dim(msg: &str) {
    eprintln!("{}", msg.dimmed());
}

pub fn format_message(m: &StoredMessage) -> String {
    let ts = m.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
    let chan = m.channel_name.as_deref().unwrap_or("");
    let chan_part = if chan.is_empty() {
        String::new()
    } else {
        format!("#{} | ", chan)
    };
    let content = m.content.replace('\n', " ");
    let content = if content.chars().count() > 300 {
        let truncated: String = content.chars().take(300).collect();
        format!("{}…", truncated)
    } else {
        content
    };
    format!(
        "{} {}{}: {}",
        ts.dimmed(),
        chan_part.cyan(),
        m.sender_name.bold(),
        content
    )
}
