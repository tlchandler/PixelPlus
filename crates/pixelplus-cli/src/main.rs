//! `pixelplus` — administration and bring-up tool for PixelPlus controllers.

mod args;
mod cmd;
mod http;
mod hwctx;
mod style;

use args::BoardArg;
use clap::{Parser, Subcommand};
use hwctx::HwContext;
use std::process::ExitCode;

const AFTER_HELP: &str = "\
Examples:
  pixelplus detect                                   identify board, Pi and sensors
  pixelplus eeprom write --board difftx --rev E      program a new board's EEPROM
  pixelplus config-txt --board difftxlarge --pixels 1600
  pixelplus test-output --pattern chase --color red --seconds 30
  pixelplus test-output --pattern scope --scope zeros --output 1
  pixelplus doctor

On a PC, add --simulate <board> to try any command against a simulated board.";

/// PixelPlus controller administration and bring-up.
#[derive(Debug, Parser)]
#[command(
    name = "pixelplus",
    version,
    about = "PixelPlus controller administration and bring-up",
    after_help = AFTER_HELP,
    propagate_version = true
)]
struct Cli {
    /// Use a simulated board instead of the real I2C bus and pins.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "BOARD",
        env = "PIXELPLUS_SIMULATE"
    )]
    simulate: Option<BoardArg>,

    /// Print machine-readable JSON instead of text (where supported).
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read, write or erase the board identity EEPROM.
    #[command(subcommand)]
    Eeprom(cmd::eeprom::EepromCmd),

    /// Identify the board and Raspberry Pi, and read every sensor.
    Detect,

    /// Drive test patterns straight to the pixel outputs (no daemon needed).
    ///
    /// Stop pixelplusd first (`sudo systemctl stop pixelplusd`): only one
    /// program can own the display output.
    TestOutput(cmd::test_output::TestOutputArgs),

    /// Print the /boot/firmware/config.txt fragment for a board.
    ConfigTxt(cmd::config_txt::ConfigTxtArgs),

    /// Show what the local pixelplusd is doing.
    Status(cmd::status::StatusArgs),

    /// Check the system for problems (overlay, I2C, sensors, audio, disk, time).
    Doctor(cmd::doctor::DoctorArgs),

    /// Switch the pixel pins to DPI or park them low (used by the systemd unit).
    Pins(cmd::pins::PinsArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = HwContext {
        simulate: cli.simulate.map(Into::into),
    };
    let json = cli.json;
    let result = match cli.command {
        Command::Eeprom(c) => cmd::eeprom::run(&ctx, c, json),
        Command::Detect => cmd::detect::run(&ctx, json),
        Command::TestOutput(a) => cmd::test_output::run(&ctx, a, json),
        Command::ConfigTxt(a) => cmd::config_txt::run(&ctx, a),
        Command::Status(a) => cmd::status::run(a, json),
        Command::Doctor(a) => cmd::doctor::run(&ctx, a, json),
        Command::Pins(a) => cmd::pins::run(&ctx, a),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            anstream::eprintln!(
                "{} {}",
                style::paint(style::FAIL, "error:"),
                error_chain(&e)
            );
            ExitCode::FAILURE
        }
    }
}

/// `a: b: c` like `{:#}`, but without repeating a cause whose text the previous
/// message already ends with (hardware errors embed their OS error).
pub fn error_chain(e: &anyhow::Error) -> String {
    let mut out = String::new();
    for cause in e.chain() {
        let text = cause.to_string();
        if out.ends_with(&text) {
            continue;
        }
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn error_chain_skips_repeats() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "No such file");
        let e = anyhow::Error::new(io).context("opening /dev/i2c-1: No such file");
        assert_eq!(error_chain(&e), "opening /dev/i2c-1: No such file");
        let e = anyhow::anyhow!("inner").context("outer");
        assert_eq!(error_chain(&e), "outer: inner");
    }

    /// firstboot (image/firstboot/firstboot.py) runs exactly these command lines.
    #[test]
    fn firstboot_contract_parses() {
        for argv in [
            vec!["pixelplus", "--json", "detect"],
            vec!["pixelplus", "config-txt", "--board", "difftxlarge"],
            vec![
                "pixelplus",
                "config-txt",
                "--board",
                "bare-pi",
                "--pixels",
                "1600",
            ],
            vec!["pixelplus", "pins", "release"],
        ] {
            Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        }
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_examples() {
        let ok = [
            "pixelplus detect",
            "pixelplus --json detect",
            "pixelplus eeprom read --hex",
            "pixelplus eeprom write --board difftx --rev E --yes",
            "pixelplus eeprom erase --full -y",
            "pixelplus config-txt --board difftxlarge --pi pi5 --pixels 1600",
            "pixelplus config-txt --board difftx --fps 40",
            "pixelplus test-output --board difftx --pattern scope --scope identify --seconds 0 --sim",
            "pixelplus test-output --pattern solid --color '#ff8000' --order grb --output 2",
            "pixelplus --simulate difftxlarge doctor",
            "pixelplus status --url http://pi.local:8080",
            "pixelplus pins release --board difftxlarge",
        ];
        for line in ok {
            let argv: Vec<String> = line
                .split(' ')
                .map(|s| s.trim_matches('\'').to_string())
                .collect();
            Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{line}: {e}"));
        }
        let bad = [
            "pixelplus config-txt --board difftx --pixels 10 --fps 40",
            "pixelplus test-output --brightness 101",
            "pixelplus test-output --color purple-ish",
            "pixelplus eeprom write --board toaster --rev A",
        ];
        for line in bad {
            let argv: Vec<&str> = line.split(' ').collect();
            assert!(Cli::try_parse_from(argv).is_err(), "{line} should fail");
        }
    }
}
