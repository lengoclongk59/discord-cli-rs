//! `discord export <CHANNEL>` — export messages to file.

use std::fs;
use std::io::{self, BufWriter, Write};

use anyhow::Result;

use crate::cli::{ExportArgs, ExportFormat};
use crate::commands::resolve;
use crate::commands::Ctx;
use crate::db::Db;
use crate::output;

pub fn run(ctx: &Ctx, args: ExportArgs) -> Result<()> {
    let db = Db::open(&ctx.db_path)?;

    let channel_id = resolve::resolve_channel_required(&db, &args.channel)?;

    let msgs = db.recent(Some(&channel_id), args.hours, 100_000)?;

    if msgs.is_empty() {
        output::dim("No messages to export.");
        return Ok(());
    }

    let mut writer: BufWriter<Box<dyn Write>> = if let Some(path) = &args.output {
        BufWriter::new(Box::new(fs::File::create(path)?))
    } else {
        BufWriter::new(Box::new(io::stdout().lock()))
    };

    match args.format {
        ExportFormat::Json => {
            serde_json::to_writer_pretty(&mut writer, &msgs)?;
            writeln!(writer)?;
        }
        ExportFormat::Yaml => {
            serde_yaml::to_writer(&mut writer, &msgs)?;
        }
        ExportFormat::Text => {
            for m in &msgs {
                let ts = m.timestamp.format("%Y-%m-%d %H:%M:%S");
                writeln!(writer, "[{}] {}: {}", ts, m.sender_name, m.content.replace('\n', " "))?;
            }
        }
    }

    writer.flush()?;

    if let Some(path) = &args.output {
        output::success(&format!("Exported {} messages to {}", msgs.len(), path.display()));
    }
    Ok(())
}
