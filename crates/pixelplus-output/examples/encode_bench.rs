//! Encoder throughput benchmark.
//!
//! ```text
//! cargo run --release -p pixelplus-output --example encode_bench [pixels]
//! ```
//!
//! Encodes 60 × `pixels` (default 1600) LEDs for the difftxlarge (latched)
//! and 4 × `pixels` for the difftx (direct) into an in-memory framebuffer and
//! prints the time per frame for the first (full) and steady-state
//! (incremental) encodes.

use pixelplus_core::model::BoardKind;
use pixelplus_output::{BufferState, DpiGeometry, FrameBufferMut, OutputFrameRef, OutputLayout, WsEncoder};
use std::time::Instant;

fn bench(board: BoardKind, pixels: u32) -> Result<(), pixelplus_output::OutputError> {
    let layout = OutputLayout::for_board(board);
    let outputs = layout.output_count();
    let geometry = DpiGeometry::for_pixels(pixels)?;
    let encoder = WsEncoder::new(layout, geometry)?;
    let data: Vec<Vec<u8>> = (0..outputs)
        .map(|o| {
            (0..pixels as usize * 3)
                .map(|i| (i.wrapping_mul(31) ^ o.wrapping_mul(97)) as u8)
                .collect()
        })
        .collect();
    let frame = OutputFrameRef::new(data.iter().map(Vec::as_slice).collect());
    let (w, h) = (geometry.hactive() as usize, geometry.vactive() as usize);
    let mut words = vec![0u32; w * h];
    let mut state = BufferState::new();

    let t = Instant::now();
    {
        let mut fb = FrameBufferMut::new(&mut words, w, h, w)?;
        encoder.encode(&frame, &mut fb, &mut state)?;
    }
    let first = t.elapsed();

    let rounds = 50;
    let t = Instant::now();
    for _ in 0..rounds {
        let mut fb = FrameBufferMut::new(&mut words, w, h, w)?;
        encoder.encode(&frame, &mut fb, &mut state)?;
    }
    let steady = t.elapsed() / rounds;
    println!(
        "{:<12} {:>2} outputs × {pixels} px: first frame {:>7.2} ms, steady {:>6.2} ms/frame \
         ({:.1} MB framebuffer, display refresh {:.1} Hz)",
        format!("{board:?}"),
        outputs,
        first.as_secs_f64() * 1e3,
        steady.as_secs_f64() * 1e3,
        (w * h * 4) as f64 / 1e6,
        geometry.refresh_hz()
    );
    Ok(())
}

fn main() -> Result<(), pixelplus_output::OutputError> {
    let pixels = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1600);
    bench(BoardKind::Difftxlarge, pixels)?;
    bench(BoardKind::Difftx, pixels)?;
    Ok(())
}
