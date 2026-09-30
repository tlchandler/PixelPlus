//! `pixelplus eeprom read | write | erase`.

use crate::args::BoardArg;
use crate::hwctx::HwContext;
use crate::style::{self, paint, Level};
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use pixelplus_core::model::BoardKind;
use pixelplus_hw::board::board_warnings;
use pixelplus_hw::eeprom::{self, EepromContents, Ppx1Record};
use std::io::{BufRead, IsTerminal, Write};

/// EEPROM operations.
#[derive(Debug, Subcommand)]
pub enum EepromCmd {
    /// Show what the board EEPROM holds.
    Read {
        /// Also dump the first 64 bytes in hex.
        #[arg(long)]
        hex: bool,
    },
    /// Write a PixelPlus (PPX1) record identifying the board.
    ///
    /// Open the board's write-protect jumper (JP1) first if it has one.
    Write(WriteArgs),
    /// Erase the EEPROM (the board becomes "unknown" until written again).
    Erase {
        /// Erase all 32 KiB instead of just the record.
        #[arg(long)]
        full: bool,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Arguments of `eeprom write`.
#[derive(Debug, Args)]
pub struct WriteArgs {
    /// Board type.
    #[arg(long, value_enum)]
    pub board: BoardArg,
    /// Board revision letter as printed on the PCB (e.g. E).
    #[arg(long)]
    pub rev: String,
    /// Serial number [default: a new random PPX-XXXXXXXX].
    #[arg(long)]
    pub serial: Option<String>,
    /// Manufacturing date YYYY-MM-DD [default: today].
    #[arg(long)]
    pub made: Option<String>,
    /// Free-text note stored with the record.
    #[arg(long)]
    pub notes: Option<String>,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Run an EEPROM subcommand.
pub fn run(ctx: &HwContext, cmd: EepromCmd, json: bool) -> Result<()> {
    match cmd {
        EepromCmd::Read { hex } => read(ctx, hex, json),
        EepromCmd::Write(args) => write(ctx, args, json),
        EepromCmd::Erase { full, yes } => erase(ctx, full, yes, json),
    }
}

fn describe(contents: &EepromContents) {
    match contents {
        EepromContents::Ppx1 { record } => {
            let board = record
                .board_kind()
                .map(|b| b.display_name().to_string())
                .unwrap_or_else(|| format!("{} (unknown to this version)", record.board));
            anstream::println!("{} PixelPlus record (PPX1)", Level::Ok.badge());
            style::kv("Board", board, 8);
            style::kv("Revision", &record.rev, 8);
            style::kv("Serial", record.serial.as_deref().unwrap_or("-"), 8);
            style::kv("Made", record.made.as_deref().unwrap_or("-"), 8);
            if let Some(notes) = &record.notes {
                style::kv("Notes", notes, 8);
            }
            if let Some(b) = record.board_kind() {
                for w in board_warnings(b, Some(&record.rev)) {
                    anstream::println!("{} {w}", Level::Warn.badge());
                }
            }
        }
        EepromContents::Blank => anstream::println!(
            "{} Blank. Identify the board with `pixelplus eeprom write --board <board> --rev <rev>`.",
            Level::Warn.badge()
        ),
        EepromContents::Fpp {
            cape,
            version,
            serial,
        } => {
            anstream::println!("{} FPP cape image (FPP02)", Level::Ok.badge());
            style::kv("Cape", cape, 8);
            style::kv("Version", version, 8);
            style::kv("Serial", serial, 8);
        }
        EepromContents::Unknown { head } => anstream::println!(
            "{} Unrecognised contents (starts {})",
            Level::Warn.badge(),
            hex_bytes(head)
        ),
        EepromContents::Corrupt { reason } => {
            anstream::println!("{} Corrupt PPX1 record: {reason}", Level::Fail.badge())
        }
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn read(ctx: &HwContext, hex: bool, json: bool) -> Result<()> {
    let mut store = ctx.eeprom().context("opening the board EEPROM")?;
    let contents = eeprom::read_contents(store.as_mut())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&contents)?);
        return Ok(());
    }
    describe(&contents);
    if hex {
        let mut head = vec![0u8; 64.min(store.size())];
        store.read(0, &mut head)?;
        anstream::println!();
        for (i, row) in head.chunks(16).enumerate() {
            let ascii: String = row
                .iter()
                .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
                .collect();
            anstream::println!(
                "  {}  {:<47}  {ascii}",
                paint(style::DIM, format!("{:04x}", i * 16)),
                hex_bytes(row)
            );
        }
    }
    Ok(())
}

/// Ask a yes/no question on the terminal; refuses when stdin is not a TTY.
pub fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("{question} — refusing without a terminal; pass --yes");
    }
    anstream::print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn write(ctx: &HwContext, args: WriteArgs, json: bool) -> Result<()> {
    let board = BoardKind::from(args.board);
    if matches!(board, BoardKind::BarePi | BoardKind::Virtual) {
        bail!("{} has no board EEPROM", board.display_name());
    }
    let mut record = Ppx1Record::new(board, &args.rev);
    if let Some(serial) = args.serial {
        record.serial = Some(serial);
    }
    if let Some(made) = args.made {
        record.made = Some(made);
    }
    record.notes = args.notes;
    record.validate()?;

    let mut store = ctx.eeprom().context("opening the board EEPROM")?;
    let current = eeprom::read_contents(store.as_mut())?;
    if !args.yes {
        if !json {
            anstream::println!("{}", paint(style::HEADING, "Currently:"));
            describe(&current);
            anstream::println!();
        }
        let q = format!(
            "Write {} rev {} (serial {})?",
            board.display_name(),
            record.rev,
            record.serial.as_deref().unwrap_or("-")
        );
        if !confirm(&q)? {
            bail!("cancelled");
        }
    }
    eeprom::write_record(store.as_mut(), &record)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&EepromContents::Ppx1 { record })?
        );
    } else {
        anstream::println!("{} EEPROM written and verified.", Level::Ok.badge());
        for w in board_warnings(board, Some(&record.rev)) {
            anstream::println!("{} {w}", Level::Warn.badge());
        }
    }
    Ok(())
}

fn erase(ctx: &HwContext, full: bool, yes: bool, json: bool) -> Result<()> {
    let mut store = ctx.eeprom().context("opening the board EEPROM")?;
    if !yes && !confirm("Erase the board EEPROM?")? {
        bail!("cancelled");
    }
    eeprom::erase(store.as_mut(), full)?;
    if json {
        println!("{}", serde_json::json!({ "erased": true, "full": full }));
    } else {
        anstream::println!("{} EEPROM erased.", Level::Ok.badge());
    }
    Ok(())
}
