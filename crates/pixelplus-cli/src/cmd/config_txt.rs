//! `pixelplus config-txt`: the boot configuration fragment for a board.

use crate::args::{BoardArg, PiArg};
use crate::hwctx::HwContext;
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

/// Run `config-txt` (prints the fragment).
pub fn run(ctx: &HwContext, args: ConfigTxtArgs) -> Result<()> {
    print!("{}", render(ctx, &args)?);
    Ok(())
}

/// The fragment (or overlay source) `config-txt` prints.
pub fn render(ctx: &HwContext, args: &ConfigTxtArgs) -> Result<String> {
    let board = BoardKind::from(args.board);
    let soc = match args.pi.soc() {
        Some(s) => s,
        None => {
            // --simulate: the simulated Pi (see HwContext::pi_info).
            let model = if ctx.is_simulated() {
                ctx.pi_info().map(|p| p.model)
            } else {
                read_pi_info().map(|p| p.model)
            };
            match model.as_deref().and_then(DpiSoc::from_model) {
                Some(s) => s,
                None => bail!("this is not a Raspberry Pi; choose one with --pi pi3|pi4|pi5"),
            }
        }
    };
    if args.overlay {
        return Ok(soc.overlay_source().to_string());
    }
    let geometry = match (args.pixels, args.fps) {
        (_, Some(fps)) => DpiGeometry::for_refresh(fps)?,
        (Some(px), None) => DpiGeometry::for_pixels(px)?,
        (None, None) => DpiGeometry::for_pixels(800)?,
    };
    Ok(config_txt(board, soc, &geometry)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hwctx::HwContext;

    fn args(board: BoardArg, pixels: Option<u32>) -> ConfigTxtArgs {
        ConfigTxtArgs {
            board,
            pi: PiArg::Auto,
            pixels,
            fps: None,
            overlay: false,
        }
    }

    #[test]
    fn simulated_pi_needs_no_pi_flag() {
        let ctx = HwContext {
            simulate: Some(BoardKind::Difftx),
        };
        let f = render(&ctx, &args(BoardArg::Difftx, Some(1600))).unwrap();
        assert!(f.contains("up to 1600 pixels per output"), "{f}");
        assert!(f.contains("dtoverlay=pixelplus-dpi"));
        // Bare Pi / virtual: still a valid fragment (I2C for sensors), no pixel engine.
        let f = render(&ctx, &args(BoardArg::BarePi, None)).unwrap();
        assert!(f.contains("dtparam=i2c_arm=on"));
    }
}
