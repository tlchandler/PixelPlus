//! Output tap for development and end-to-end tests (`GET /api/v1/debug/output`).
//!
//! When enabled (`PIXELPLUS_DEV=1` or `PIXELPLUS_OUTPUT=sim`) the output thread
//! copies every frame it writes into an [`OutputTap`]: the rendered RGB per output
//! (before colour order, brightness and gamma), the bytes the simulated output
//! received (after them), and which sequence frame was shown. Tests use it to
//! prove that the leader and every follower show the same frame at the same time.

use parking_lot::Mutex;
use serde::Serialize;

/// What was on the outputs after the last frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TapFrame {
    /// Frames written since the engine started.
    pub frame_no: u64,
    /// Engine clock (ms since the daemon started) when the frame was written.
    pub at_ms: f64,
    /// Wall clock (ms since the Unix epoch) when the frame was written.
    pub wall_ms: i64,
    /// Sequence being shown and the frame index read from it.
    pub sequence: Option<(String, u32)>,
    /// Master brightness applied (0..100, includes fades and blackout).
    pub master: u8,
    pub pixels_per_output: Vec<u32>,
    /// Rendered RGB, output-major (colour order not applied).
    pub rgb: Vec<u8>,
    /// What the output backend received per output (colour order, brightness and gamma applied).
    pub wire: Vec<Vec<u8>>,
}

/// Shared between the output thread (writer) and the API (reader).
#[derive(Debug, Default)]
pub struct OutputTap {
    last: Mutex<TapFrame>,
}

impl OutputTap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by the output thread after every frame.
    pub fn record(
        &self,
        frame_no: u64,
        at_ms: f64,
        sequence: Option<(&str, u32)>,
        master: u8,
        ppo: &[u32],
        rgb: &[u8],
        wire: &mut dyn Iterator<Item = &[u8]>,
    ) {
        let mut t = self.last.lock();
        t.frame_no = frame_no;
        t.at_ms = at_ms;
        t.wall_ms = chrono::Utc::now().timestamp_millis();
        t.sequence = sequence.map(|(id, f)| (id.to_string(), f));
        t.master = master;
        t.pixels_per_output.clear();
        t.pixels_per_output.extend_from_slice(ppo);
        t.rgb.clear();
        t.rgb.extend_from_slice(rgb);
        let mut n = 0;
        for w in wire {
            if t.wire.len() <= n {
                t.wire.push(Vec::new());
            }
            t.wire[n].clear();
            t.wire[n].extend_from_slice(w);
            n += 1;
        }
        t.wire.truncate(n);
    }

    pub fn snapshot(&self) -> TapFrame {
        self.last.lock().clone()
    }
}

/// JSON form of one output.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TapOutput {
    /// 1-based output index.
    pub index: usize,
    pub pixels: u32,
    /// Rendered RGB (base64).
    pub rgb: String,
    /// Bytes the output backend received, wire order (base64).
    pub wire: String,
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Split the tapped frame into per-output JSON entries.
pub fn outputs(frame: &TapFrame) -> Vec<TapOutput> {
    let mut off = 0usize;
    frame
        .pixels_per_output
        .iter()
        .enumerate()
        .map(|(i, &px)| {
            let len = px as usize * 3;
            let rgb = frame.rgb.get(off..off + len).unwrap_or(&[]);
            off += len;
            TapOutput {
                index: i + 1,
                pixels: px,
                rgb: base64(rgb),
                wire: base64(frame.wire.get(i).map(Vec::as_slice).unwrap_or(&[])),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn outputs_split_the_frame_per_output() {
        let tap = OutputTap::new();
        let wire: [&[u8]; 2] = [&[3, 2, 1], &[6, 5, 4, 9, 8, 7]];
        tap.record(
            7,
            1.0,
            Some(("s", 3)),
            100,
            &[1, 2],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9],
            &mut wire.iter().copied(),
        );
        let f = tap.snapshot();
        assert_eq!(f.sequence, Some(("s".into(), 3)));
        let o = outputs(&f);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].rgb, base64(&[1, 2, 3]));
        assert_eq!(o[1].rgb, base64(&[4, 5, 6, 7, 8, 9]));
        assert_eq!(o[1].index, 2);
        assert_eq!(o[0].wire, base64(&[3, 2, 1]));
        assert_eq!(o[1].wire, base64(&[6, 5, 4, 9, 8, 7]));
    }
}
