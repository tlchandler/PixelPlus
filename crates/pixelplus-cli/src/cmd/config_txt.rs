//! `pixelplus config-txt`: the boot configuration fragment for a board.

use crate::args::{BoardArg, PiArg};
use anyhow::{bail, Result};
use clap::Args;
use pixelplus_core::model::BoardKind;
use pixelplus_hw::board::read_pi_info;
use pixelplus_output::pi_config::{config_txt, DpiSoc};
use pixelplus_output::DpiGeometry;

/// Arguments of `config-txt`.
#[derive(Debug, Args)]
pub struct ConfigTxtArgs {
    /// Board.
    #[arg(long, value_enum)]
    pub board: BoardArg,
    /// Raspberry Pi generation.
    #[arg(long, value_enum, default_value_t = PiArg::Auto)]
    pub pi: PiArg,
    /// Longest string (LEDs per output) to support [default: 800, ≈40 fps].
    #[arg(long, conflicts_with = "fps", value_parser = clap::value_parser!(u32).range(1..=16384))]
    pub pixels: Option<u32>,
    /// Instead of --pixels: the frame rate to guarantee; strings get as long as that allows.
    #[arg(long)]
    pub fps: Option<f64>,
    /// Print the device-tree overlay source (.dts) instead of the config.txt fragment.
    #[arg(long)]
    pub overlay: bool,
}

/// Run `config-txt`.
pub fn run(args: ConfigTxtArgs) -> Result<()> {
    let board = BoardKind::from(args.board);
    let soc = match args.pi.soc() {
        Some(s) => s,
        None => {
            let model = read_pi_info().map(|p| p.model);
            match model.as_deref().and_then(DpiSoc::from_model) {
                Some(s) => s,
                None => bail!("this is not a Raspberry Pi; choose one with --pi pi3|pi4|pi5"),
            }
        }
    };
    if args.overlay {
        print!("{}", soc.overlay_source());
        return Ok(());
    }
    let geometry = match (args.pixels, args.fps) {
        (_, Some(fps)) => DpiGeometry::for_refresh(fps)?,
        (Some(px), None) => DpiGeometry::for_pixels(px)?,
        (None, None) => DpiGeometry::for_pixels(800)?,
    };
    print!("{}", config_txt(board, soc, &geometry)?);
    Ok(())
}
