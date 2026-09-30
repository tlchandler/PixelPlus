//! End-to-end: pipeline → encoder → framebuffer → independent decoder.

use pixelplus_core::model::{BoardKind, ColorOrder, OutputConfig};
use pixelplus_output::{
    BufferState, DpiGeometry, FrameBufferMut, FrameBufferRef, OutputFrame, OutputFrameRef,
    OutputLayout, PixelOutput, PixelPipeline, ScopePattern, SimOutput, TestPattern, WsDecoder,
    WsEncoder,
};
use std::time::Instant;

/// Deterministic pseudo-random bytes (xorshift), no external crates needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn random_frame(rng: &mut Rng, outputs: usize, max_pixels: usize) -> OutputFrame {
    OutputFrame {
        outputs: (0..outputs)
            .map(|_| {
                // Mix of empty, short, full-length, over-long and ragged lengths.
                let len = match rng.below(6) {
                    0 => 0,
                    1 => max_pixels * 3,
                    2 => (max_pixels + 3) * 3,
                    3 => rng.below(max_pixels * 3 + 1),
                    _ => rng.below(max_pixels + 1) * 3,
                };
                rng.bytes(len)
            })
            .collect(),
    }
}

#[test]
fn random_frames_round_trip_on_every_board() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for board in [BoardKind::Difftx, BoardKind::Diffsmart, BoardKind::Difftxlarge] {
        let layout = OutputLayout::for_board(board);
        let geometry = DpiGeometry::for_pixels(20).unwrap();
        let mut sim = SimOutput::verifying(layout.clone(), geometry).unwrap();
        sim.start().unwrap();
        for _ in 0..25 {
            let frame = random_frame(&mut rng, layout.output_count(), 20);
            sim.write_frame(&frame.as_frame_ref())
                .unwrap_or_else(|e| panic!("{board:?}: {e}"));
        }
        let stats = sim.stats();
        assert_eq!(stats.errors, 0, "{board:?}: {:?}", stats.last_error);
        assert_eq!(stats.frames, 25);
    }
}

#[test]
fn double_buffered_incremental_encoding_matches_fresh_encode() {
    // Two buffers used alternately, like the DPI backend's page flipping.
    let mut rng = Rng(42);
    let layout = OutputLayout::for_board(BoardKind::Difftxlarge);
    let geometry = DpiGeometry::for_pixels(12).unwrap();
    let enc = WsEncoder::new(layout.clone(), geometry).unwrap();
    let (w, h) = (geometry.hactive() as usize, geometry.vactive() as usize);
    let stride = w + 16; // DRM pitch is often padded
    let mut bufs = [vec![0x5555_5555u32; stride * h], vec![0xAAAA_AAAAu32; stride * h]];
    let mut states = [BufferState::new(), BufferState::new()];
    for i in 0..20 {
        let frame = random_frame(&mut rng, 60, 12);
        let k = i % 2;
        let mut fb = FrameBufferMut::new(&mut bufs[k], w, h, stride).unwrap();
        enc.encode(&frame.as_frame_ref(), &mut fb, &mut states[k]).unwrap();
        let fb = FrameBufferRef::new(&bufs[k], w, h, stride).unwrap();
        let decoded = WsDecoder::new(layout.clone(), geometry).decode(&fb).unwrap();
        decoded.verify(&frame.as_frame_ref(), 12).unwrap();
    }
}

#[test]
fn pipeline_output_decodes_to_wire_order() {
    let configs: Vec<OutputConfig> = [ColorOrder::GRB, ColorOrder::BGR, ColorOrder::RGB, ColorOrder::BRG]
        .into_iter()
        .enumerate()
        .map(|(i, order)| OutputConfig {
            index: i as u32 + 1,
            color_order: order,
            brightness: 100,
            gamma: 1.0,
            enabled: true,
            ..OutputConfig::default()
        })
        .collect();
    let pipeline = PixelPipeline::new(&configs);
    let red = [255u8, 0, 0, 10, 20, 30];
    let input = OutputFrameRef::new(vec![&red; 4]);
    let mut wire = OutputFrame::default();
    pipeline.process(&input, &mut wire);

    let layout = OutputLayout::for_board(BoardKind::Difftx);
    let geometry = DpiGeometry::for_pixels(4).unwrap();
    let words = WsEncoder::new(layout.clone(), geometry)
        .unwrap()
        .encode_to_vec(&wire.as_frame_ref())
        .unwrap();
    let fb = FrameBufferRef::new(&words, 1152, geometry.vactive() as usize, 1152).unwrap();
    let decoded = WsDecoder::new(layout, geometry).decode(&fb).unwrap();
    assert_eq!(decoded.outputs[0].bytes, vec![0, 255, 0, 20, 10, 30]); // GRB
    assert_eq!(decoded.outputs[1].bytes, vec![0, 0, 255, 30, 20, 10]); // BGR
    assert_eq!(decoded.outputs[3].bytes, vec![0, 255, 0, 30, 10, 20]); // BRG
}

#[test]
fn scope_patterns_have_the_advertised_pulses() {
    let layout = OutputLayout::for_board(BoardKind::Difftxlarge);
    let geometry = DpiGeometry::for_pixels(4).unwrap();
    let enc = WsEncoder::new(layout.clone(), geometry).unwrap();
    let dec = WsDecoder::new(layout, geometry);
    let mut frame = OutputFrame::default();
    for pattern in ScopePattern::ALL {
        TestPattern::Scope { pattern }.render(0, 60, 4, &mut frame);
        let words = enc.encode_to_vec(&frame.as_frame_ref()).unwrap();
        let fb = FrameBufferRef::new(&words, 1152, geometry.vactive() as usize, 1152).unwrap();
        let d = dec.decode(&fb).unwrap();
        d.verify(&frame.as_frame_ref(), 4).unwrap();
        match pattern {
            ScopePattern::Zeros => assert!(d.outputs.iter().all(|o| o.t1h_ns.is_none())),
            ScopePattern::Ones => assert!(d.outputs.iter().all(|o| o.t0h_ns.is_none())),
            ScopePattern::Identify => assert_eq!(d.outputs[41].bytes[0], 42),
            _ => {}
        }
    }
}

#[test]
fn full_size_difftxlarge_frame_encodes_and_decodes() {
    // 60 outputs × 1600 LEDs — the ARCHITECTURE §3.4 practical maximum.
    let mut rng = Rng(7);
    let layout = OutputLayout::for_board(BoardKind::Difftxlarge);
    let geometry = DpiGeometry::for_pixels(1600).unwrap();
    let frame = OutputFrame {
        outputs: (0..60).map(|_| rng.bytes(1600 * 3)).collect(),
    };
    let enc = WsEncoder::new(layout.clone(), geometry).unwrap();
    let started = Instant::now();
    let words = enc.encode_to_vec(&frame.as_frame_ref()).unwrap();
    let elapsed = started.elapsed();
    let fb = FrameBufferRef::new(&words, 1152, geometry.vactive() as usize, 1152).unwrap();
    let decoded = WsDecoder::new(layout, geometry).decode(&fb).unwrap();
    decoded.verify(&frame.as_frame_ref(), 1600).unwrap();
    assert!(geometry.refresh_hz() >= 20.0);
    // Generous bound so unoptimised debug builds pass; see examples/encode_bench.rs
    // for release numbers.
    assert!(elapsed.as_millis() < 5_000, "encode took {elapsed:?}");
}
