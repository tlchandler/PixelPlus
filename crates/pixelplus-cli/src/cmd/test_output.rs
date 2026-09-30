//! `pixelplus test-output`: drive test patterns straight to the pixel
//! outputs, without the daemon (bring-up, scope checks, wiring checks).

use crate::args::{parse_color, BoardArg, OrderArg};
use crate::hwctx::HwContext;
use crate::style::{self, paint, Level};
use anyhow::{bail, Context, Result};
use clap::{Args, ValueEnum};
use pixelplus_core::model::{BoardKind, OutputConfig};
use pixelplus_output::{
    DpiGeometry, OutputFrame, OutputLayout, PixelOutput, PixelPipeline, ScopePattern, SimOutput,
    TestPattern,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Which pattern to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PatternArg {
    /// Every 4th LED lit, moving along the string.
    Chase,
    /// Every LED one colour (--color).
    Solid,
    /// Whole strings step red, green, blue, white every second.
    Rgb,
    /// Raw wire pattern for an oscilloscope (--scope).
    Scope,
}

/// Oscilloscope waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScopeArg {
    /// 0x00 bytes: only 312 ns pulses.
    Zeros,
    /// 0xFF bytes: only 703 ns pulses.
    Ones,
    /// 0xAA bytes: long and short pulses alternating.
    Alternating,
    /// LEDs alternate all-0 / all-1.
    Checker,
    /// First byte on each output is its output number.
    Identify,
}

impl From<ScopeArg> for ScopePattern {
    fn from(s: ScopeArg) -> Self {
        match s {
            ScopeArg::Zeros => ScopePattern::Zeros,
            ScopeArg::Ones => ScopePattern::Ones,
            ScopeArg::Alternating => ScopePattern::Alternating,
            ScopeArg::Checker => ScopePattern::Checker,
            ScopeArg::Identify => ScopePattern::Identify,
        }
    }
}

/// Arguments of `test-output`.
#[derive(Debug, Args)]
pub struct TestOutputArgs {
    /// Board [default: detected from the EEPROM].
    #[arg(long, value_enum)]
    pub board: Option<BoardArg>,
    /// Pattern to show.
    #[arg(long, value_enum, default_value_t = PatternArg::Chase)]
    pub pattern: PatternArg,
    /// Waveform for --pattern scope.
    #[arg(long, value_enum, default_value_t = ScopeArg::Alternating)]
    pub scope: ScopeArg,
    /// Colour for solid and chase: hex (ff8000) or a name (red, warm, ...).
    #[arg(long, default_value = "white", value_parser = parse_color)]
    pub color: [u8; 3],
    /// LEDs per output.
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=16384))]
    pub pixels: u32,
    /// Brightness in percent (kept low by default to spare power supplies).
    #[arg(long, default_value_t = 25, value_parser = clap::value_parser!(u8).range(0..=100))]
    pub brightness: u8,
    /// Colour order of the pixels.
    #[arg(long, value_enum, default_value_t = OrderArg::Rgb, ignore_case = true)]
    pub order: OrderArg,
    /// How long to run; 0 runs until Ctrl-C.
    #[arg(long, default_value_t = 10.0)]
    pub seconds: f64,
    /// Only drive this output (1-based); the others are sent black.
    #[arg(long)]
    pub output: Option<usize>,
    /// Simulate instead of driving the pins: every frame is encoded and
    /// decoded to verify data and WS281x timing.
    #[arg(long)]
    pub sim: bool,
}

#[cfg(target_os = "linux")]
fn install_interrupt_handler() {
    extern "C" fn on_signal(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
}

#[cfg(not(target_os = "linux"))]
fn install_interrupt_handler() {}

fn pattern(args: &TestOutputArgs) -> (TestPattern, Duration) {
    match args.pattern {
        PatternArg::Chase => (
            TestPattern::Chase {
                color: args.color,
                spacing: 4,
            },
            Duration::from_millis(80),
        ),
        PatternArg::Solid => (
            TestPattern::Solid { color: args.color },
            Duration::from_secs(3600),
        ),
        PatternArg::Rgb => (TestPattern::RgbCycle, Duration::from_secs(1)),
        PatternArg::Scope => (
            TestPattern::Scope {
                pattern: args.scope.into(),
            },
            Duration::from_secs(3600),
        ),
    }
}

fn open_backend(
    ctx: &HwContext,
    args: &TestOutputArgs,
    board: BoardKind,
) -> Result<(Box<dyn PixelOutput>, Option<&'static str>)> {
    let layout = OutputLayout::for_board(board);
    if args.sim || ctx.is_simulated() {
        let geometry = DpiGeometry::for_pixels(args.pixels)?;
        return Ok((
            Box::new(SimOutput::verifying(layout, geometry)?),
            Some("simulated"),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let out = pixelplus_output::DpiOutput::new(pixelplus_output::DpiConfig::for_board(board));
        Ok((Box::new(out), None))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = layout;
        bail!("DPI output needs Linux on a Raspberry Pi; use --sim")
    }
}

/// Run `test-output`.
pub fn run(ctx: &HwContext, args: TestOutputArgs, json: bool) -> Result<()> {
    let board = ctx.board_or_detect(args.board.map(Into::into))?;
    let layout = OutputLayout::for_board(board);
    let outputs = layout.output_count();
    if outputs == 0 {
        bail!("{} has no pixel outputs", board.display_name());
    }
    if let Some(o) = args.output {
        if o == 0 || o > outputs {
            bail!(
                "--output must be 1..={outputs} on the {}",
                board.display_name()
            );
        }
    }
    if !(args.seconds.is_finite() && args.seconds >= 0.0) {
        bail!("--seconds must be a non-negative number");
    }

    let (mut backend, mode) = open_backend(ctx, &args, board)?;
    backend.start().context("starting pixel output")?;
    if let Some(max) = backend.stats().max_pixels_per_output {
        if args.pixels > max {
            backend.stop();
            bail!(
                "--pixels {} exceeds the {} LEDs per output the display mode allows; \
                 regenerate config.txt with `pixelplus config-txt --pixels {}` and reboot",
                args.pixels,
                max,
                args.pixels
            );
        }
    }
    install_interrupt_handler();

    let configs: Vec<OutputConfig> = (1..=outputs)
        .map(|i| OutputConfig {
            index: i as u32,
            label: board.output_label(i),
            color_order: args.order.into(),
            brightness: args.brightness,
            gamma: 1.0,
            enabled: args.output.map_or(true, |only| only == i),
            ..OutputConfig::default()
        })
        .collect();
    let pipeline = PixelPipeline::new(&configs);
    let (pattern, step) = pattern(&args);
    if !json {
        let target = match args.output {
            Some(o) => format!("output {} ({})", o, board.output_label(o)),
            None => format!("all {outputs} outputs"),
        };
        let what = match pattern {
            TestPattern::Scope { pattern } => format!("scope pattern `{}`", pattern.name()),
            _ => format!("{:?}", args.pattern).to_lowercase(),
        };
        anstream::println!(
            "{} {what} on {target}, {} LEDs each{}. {}",
            paint(style::HEADING, "Testing"),
            args.pixels,
            mode.map(|m| format!(" ({m})")).unwrap_or_default(),
            paint(
                style::DIM,
                if args.seconds == 0.0 {
                    "Ctrl-C to stop."
                } else {
                    "Ctrl-C stops early."
                }
            )
        );
        if let TestPattern::Scope { pattern } = pattern {
            anstream::println!(
                "  {}",
                paint(style::DIM, format!("Expect: {}", pattern.describe()))
            );
        }
    }

    let frame_period = Duration::from_millis(25);
    let started = Instant::now();
    let run_for = Duration::from_secs_f64(args.seconds.min(1e9));
    let mut raw = OutputFrame::default();
    let mut wire = OutputFrame::default();
    let mut result = Ok(());
    while !INTERRUPTED.load(Ordering::SeqCst) {
        let elapsed = started.elapsed();
        if args.seconds > 0.0 && elapsed >= run_for {
            break;
        }
        let tick = Instant::now();
        let n = (elapsed.as_millis() / step.as_millis().max(1)) as u64;
        pattern.render(n, outputs, args.pixels as usize, &mut raw);
        let frame = if pattern.is_wire_level() {
            if let Some(only) = args.output {
                for (i, o) in raw.outputs.iter_mut().enumerate() {
                    if i + 1 != only {
                        o.fill(0);
                    }
                }
            }
            &raw
        } else {
            pipeline.process(&raw.as_frame_ref(), &mut wire);
            &wire
        };
        if let Err(e) = backend.write_frame(&frame.as_frame_ref()) {
            result = Err(anyhow::Error::new(e).context("writing a frame"));
            break;
        }
        if let Some(rest) = frame_period.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    let stats = backend.stats();
    backend.stop();
    result?;

    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        anstream::println!(
            "{} {} frames, {} {:.2} ms average / {:.2} ms max{}. Outputs blanked.",
            Level::Ok.badge(),
            stats.frames,
            if mode.is_some() {
                "encode + verify"
            } else {
                "encode"
            },
            stats.avg_encode_us / 1000.0,
            stats.max_encode_us as f64 / 1000.0,
            stats
                .refresh_hz
                .map(|hz| format!(", display {hz:.1} Hz"))
                .unwrap_or_default()
        );
        if mode.is_some() {
            anstream::println!(
                "  {}",
                paint(style::DIM, "Simulation: every frame was decoded and matched, with WS281x timing inside spec.")
            );
        }
    }
    Ok(())
}
