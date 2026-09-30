//! `pixelplus pins claim | release`: switch the board's pixel pins between
//! the DPI function and idle-low GPIO outputs.
//!
//! `pixelplusd` does this itself; the command exists for bring-up and for the
//! systemd unit's `ExecStopPost=` so the lines are parked low even if the
//! daemon is killed.

use crate::args::BoardArg;
use crate::hwctx::HwContext;
use crate::style::Level;
use anyhow::{bail, Result};
use clap::{Args, ValueEnum};
use pixelplus_output::pi_config::gpio_ranges;
use pixelplus_output::OutputLayout;

/// What to do with the pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PinsAction {
    /// Route the pins to the DPI block (pixels follow the framebuffer).
    Claim,
    /// Make the pins GPIO outputs driven low (WS281x idle).
    Release,
}

/// Arguments of `pins`.
#[derive(Debug, Args)]
pub struct PinsArgs {
    /// claim or release.
    #[arg(value_enum)]
    pub action: PinsAction,
    /// Board [default: detected from the EEPROM].
    #[arg(long, value_enum)]
    pub board: Option<BoardArg>,
}

/// Run `pins`.
pub fn run(ctx: &HwContext, args: PinsArgs) -> Result<()> {
    let board = ctx.board_or_detect(args.board.map(Into::into))?;
    let pins = OutputLayout::for_board(board).gpio_pins();
    if pins.is_empty() {
        bail!("{} has no pixel pins", board.display_name());
    }
    if ctx.is_simulated() {
        anstream::println!(
            "{} (simulated) would {:?} GPIO {}",
            Level::Ok.badge(),
            args.action,
            gpio_ranges(&pins)
        );
        return Ok(());
    }
    apply(args.action, &pins)?;
    let what = match args.action {
        PinsAction::Claim => "switched to DPI",
        PinsAction::Release => "parked low",
    };
    anstream::println!("{} GPIO {} {what}.", Level::Ok.badge(), gpio_ranges(&pins));
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply(action: PinsAction, pins: &[u8]) -> Result<()> {
    let soc = pixelplus_output::detect_soc()?;
    let mux = pixelplus_output::pinmux::PinMux::detect(soc)?;
    match action {
        PinsAction::Claim => mux.set_dpi(pins)?,
        PinsAction::Release => mux.set_idle(pins)?,
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn apply(_: PinsAction, _: &[u8]) -> Result<()> {
    bail!("pin control needs Linux on a Raspberry Pi")
}
