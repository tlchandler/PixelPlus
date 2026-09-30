//! Bounded randomized robustness tests ("fuzz-lite") for everything that parses files
//! users or other nodes hand us: `.fseq`, `.ppseq`, `xlights_rgbeffects.xml` and
//! `xlights_networks.xml`.
//!
//! Each test mutates realistic seed files with a deterministic PRNG for a fixed time
//! budget and asserts that parsing and reading never panics (a panic fails the test).
//! Memory is bounded by the parsers' own limits (see `fseq::MAX_FRAME_BYTES` /
//! `MAX_BLOCK_BYTES`); a hostile header that made them allocate gigabytes would abort
//! the test process.
//!
//! Set `PIXELPLUS_FUZZ_SECS` to run longer (e.g. overnight on a dev box).

use std::io::Cursor;
use std::time::{Duration, Instant};

use pixelplus_core::fseq::{Compression, FseqFile, FseqWriter, FseqWriterOptions, SparseRange};
use pixelplus_core::model::Show;
use pixelplus_core::ppseq::{PpseqFile, PpseqHeader, PpseqWriter};
use pixelplus_core::xlights::{import_preview, Networks};

const RGB: &str = include_str!("../testdata/data_xlights_rgbeffects.xml");
const RGB_2025: &str = include_str!("../testdata/xlights_2025_rgbeffects.xml");
const NET: &str = include_str!("../testdata/data_xlights_networks.xml");
const NET_2025: &str = include_str!("../testdata/xlights_2025_networks.xml");

/// xorshift64*: tiny, deterministic, good enough for mutation.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let extra = std::env::var("PIXELPLUS_FUZZ_SEED")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        Rng((seed ^ extra.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

fn budget() -> Duration {
    let secs = std::env::var("PIXELPLUS_FUZZ_SECS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.5);
    Duration::from_secs_f64(secs)
}

const INTERESTING_U32: &[u32] = &[
    0,
    1,
    2,
    3,
    7,
    8,
    255,
    256,
    0xFFFF,
    0x1_0000,
    0xFF_FFFF,
    0x100_0000,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFE,
    0xFFFF_FFFF,
];

/// Mutate a binary file: byte flips, interesting integers at random offsets
/// (biased towards the header), truncation, extension, chunk duplication.
fn mutate_bytes(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut v = seed.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        let hot = v.len().clamp(1, 96);
        match rng.below(7) {
            0 | 1 => {
                if !v.is_empty() {
                    let i = if rng.below(3) == 0 {
                        rng.below(v.len())
                    } else {
                        rng.below(hot.min(v.len()))
                    };
                    v[i] ^= 1 << rng.below(8);
                }
            }
            2 | 3 => {
                let val = *rng.pick(INTERESTING_U32);
                let width = *rng.pick(&[1usize, 2, 3, 4, 8]);
                if v.len() > width {
                    let i = rng.below(hot.min(v.len() - width));
                    let bytes = (val as u64 | if width == 8 { (val as u64) << 32 } else { 0 })
                        .to_le_bytes();
                    v[i..i + width].copy_from_slice(&bytes[..width]);
                }
            }
            4 => {
                let n = rng.below(v.len() + 1);
                v.truncate(n);
            }
            5 => {
                let n = rng.below(512);
                for _ in 0..n {
                    v.push(rng.next() as u8);
                }
            }
            _ => {
                if v.len() > 8 {
                    let a = rng.below(v.len());
                    let b = (a + rng.below(64)).min(v.len());
                    let chunk = v[a..b].to_vec();
                    let at = rng.below(v.len());
                    v.splice(at..at, chunk);
                }
            }
        }
    }
    v
}

fn pattern(frame: u32, n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u32 ^ frame.wrapping_mul(13)) as u8)
        .collect()
}

fn fseq_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for (compression, sparse, fpb) in [
        (Compression::Zstd, false, 4),
        (Compression::Zlib, false, 3),
        (Compression::None, false, 1),
        (Compression::Zstd, true, 5),
        (Compression::None, true, 1),
    ] {
        let mut o = FseqWriterOptions::new(96, 25);
        o.compression = compression;
        o.frames_per_block = fpb;
        o.max_blocks = 8;
        o.media_filename = Some("C:\\show\\song.mp3".into());
        if sparse {
            o.sparse_ranges = vec![
                SparseRange { start: 3, len: 30 },
                SparseRange { start: 60, len: 12 },
            ];
        }
        let mut w = FseqWriter::new(Cursor::new(Vec::new()), o).unwrap();
        for f in 0..12 {
            w.write_frame(&pattern(f, 96)).unwrap();
        }
        seeds.push(w.finish().unwrap().into_inner());
    }
    // v1 file.
    let mut v1 = vec![0u8; 28];
    v1[0..4].copy_from_slice(b"PSEQ");
    v1[4..6].copy_from_slice(&28u16.to_le_bytes());
    v1[7] = 1;
    v1[8..10].copy_from_slice(&28u16.to_le_bytes());
    v1[10..14].copy_from_slice(&30u32.to_le_bytes());
    v1[14..18].copy_from_slice(&5u32.to_le_bytes());
    v1[18] = 50;
    for f in 0..5 {
        v1.extend(pattern(f, 30));
    }
    seeds.push(v1);
    seeds
}

fn exercise_fseq(bytes: Vec<u8>) {
    let Ok(mut f) = FseqFile::from_reader(Cursor::new(bytes)) else {
        return;
    };
    let h = f.header().clone();
    let _ = (
        h.media_basename(),
        h.producer(),
        h.duration_ms(),
        f.frame_at_ms(u64::MAX),
    );
    let size = f.frame_size().min(1 << 20);
    let mut buf = vec![0u8; size];
    let n = f.frame_count();
    for i in (0..n.min(20)).chain([n / 2, n.saturating_sub(1), n, u32::MAX]) {
        let _ = f.frame(i, &mut buf);
    }
    let mut short = [0u8; 5];
    let _ = f.frame(0, &mut short);
}

#[test]
fn fseq_mutations_never_panic() {
    let seeds = fseq_seeds();
    for s in &seeds {
        // Seeds themselves are valid.
        let mut f = FseqFile::from_reader(Cursor::new(s.clone())).unwrap();
        let mut buf = vec![0u8; f.frame_size()];
        f.frame(11.min(f.frame_count() - 1), &mut buf).unwrap();
    }
    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    let deadline = Instant::now() + budget();
    let mut iterations = 0u32;
    while Instant::now() < deadline || iterations < 200 {
        let seed = rng.pick(&seeds).clone();
        exercise_fseq(mutate_bytes(&mut rng, &seed));
        iterations += 1;
    }
    // Pure noise behind a valid magic/version.
    for _ in 0..500 {
        let len = rng.below(400);
        let mut v: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if v.len() >= 8 {
            v[0..4].copy_from_slice(b"PSEQ");
            v[7] = 1 + rng.below(2) as u8;
        }
        exercise_fseq(v);
    }
}

fn ppseq_seed() -> Vec<u8> {
    let header = PpseqHeader {
        version: 1,
        frame_count: 0,
        frame_us: 25_000,
        frame_bytes: 0,
        pixels_per_output: vec![10, 0, 6],
        source_sha256: [7; 32],
    };
    let mut w = PpseqWriter::new(Cursor::new(Vec::new()), header).unwrap();
    for f in 0..70 {
        w.write_frame(&pattern(f, 48)).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn ppseq_mutations_never_panic() {
    let seed = ppseq_seed();
    let mut pp = PpseqFile::from_reader(Cursor::new(seed.clone())).unwrap();
    let mut buf = vec![0u8; pp.frame_bytes()];
    pp.frame(69, &mut buf).unwrap();
    assert_eq!(buf, pattern(69, 48));

    let mut rng = Rng::new(0xD1B5_4A32_D192_ED03);
    let deadline = Instant::now() + budget();
    let mut iterations = 0u32;
    while Instant::now() < deadline || iterations < 200 {
        let mut v = mutate_bytes(&mut rng, &seed);
        // Also mutate the trailer/index region, which lives at the end.
        if v.len() > 40 && rng.below(2) == 0 {
            let i = v.len() - 1 - rng.below(40);
            v[i] = *rng.pick(&[0u8, 1, 0x7F, 0x80, 0xFF]);
        }
        if let Ok(mut pp) = PpseqFile::from_reader(Cursor::new(v)) {
            let mut buf = vec![0u8; pp.frame_bytes().min(1 << 20)];
            let n = pp.frame_count();
            for i in [0, 1, 63, 64, 65, n / 2, n.saturating_sub(1), n] {
                let _ = pp.frame(i, &mut buf);
            }
            let mut f = pp.new_frame();
            let _ = pp.frame_into(0, &mut f);
        }
        iterations += 1;
    }
}

const INTERESTING_ATTR: &[&str] = &[
    "",
    "0",
    "1",
    "-1",
    "2",
    "3",
    "true",
    "false",
    "B",
    "T",
    "L",
    "R",
    "Vertical",
    "Horizontal",
    "4294967295",
    "4294967296",
    "-2147483648",
    "99999999999999999999",
    "1000000",
    "1e39",
    "nan",
    "-inf",
    "abc",
    "1,2,3",
    "1,,2;,3,|4,5;,6",
    "1,0,0;2,0,1,1;3,99999,99999;4,-1,0",
    ",,,;;;|||",
    ">Arch 1:1",
    "@Matrix:-5",
    "<Nope:1",
    ">Mega Tree:99999999999",
    "!PixelPlus Leader:0",
    "!Garage F16:1",
    "#1:1",
    "#10.0.0.9:3:7",
    "#:",
    "#1.2.3.4:x:y",
    ":::",
    "WS2811",
    "DMX",
    "Tree Flat",
    "Tree Ribbon",
    "Tree 180",
    "Custom",
    "Poly Line",
    "Matrix",
    "Arches",
    "Star",
    "Icicles",
    "Spinner",
    "Window Frame",
    "Circle",
    "Sphere",
    "Cube",
    "Channel Block",
    "Label",
    "RGBW Nodes",
    "Single Color Red",
    "3 Channel RGB",
];

/// Replace random attribute values of an XML document with interesting values, and
/// occasionally cut it or duplicate an element.
fn mutate_xml(rng: &mut Rng, seed: &str) -> String {
    let starts: Vec<usize> = seed.match_indices("=\"").map(|(i, _)| i + 2).collect();
    let mut out = seed.to_string();
    let n = 1 + rng.below(6);
    let mut picks: Vec<usize> = (0..n).map(|_| *rng.pick(&starts)).collect();
    picks.sort_unstable();
    picks.dedup();
    for &at in picks.iter().rev() {
        let Some(len) = out[at..].find('"') else {
            continue;
        };
        let val = rng
            .pick(INTERESTING_ATTR)
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        out.replace_range(at..at + len, &val);
    }
    match rng.below(8) {
        0 => {
            let mut cut = rng.below(out.len());
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
        }
        1 => {
            // Duplicate a model element (duplicate names, doubled chains).
            if let Some(a) = out.find("<model ") {
                if let Some(b) = out[a..].find("/>").or_else(|| out[a..].find("</model>")) {
                    let el = out[a..a + b + 2].to_string();
                    out.insert_str(a, &el);
                }
            }
        }
        _ => {}
    }
    out
}

#[test]
fn xlights_mutations_never_panic() {
    let show = Show::default();
    // Seeds import cleanly.
    for (rgb, net) in [(RGB, NET), (RGB_2025, NET_2025)] {
        let p = import_preview(rgb, Some(net), &show).unwrap();
        assert!(!p.props.is_empty());
    }
    let mut rng = Rng::new(0x0123_4567_89AB_CDEF);
    let deadline = Instant::now() + budget();
    let mut iterations = 0u32;
    while Instant::now() < deadline || iterations < 100 {
        let (rgb, net) = *rng.pick(&[(RGB, NET), (RGB_2025, NET_2025)]);
        let rgb = mutate_xml(&mut rng, rgb);
        let net = if rng.below(3) == 0 {
            mutate_xml(&mut rng, net)
        } else {
            net.to_string()
        };
        let mut w = Vec::new();
        let _ = Networks::parse(&net, &mut w);
        if let Ok(p) = import_preview(&rgb, Some(&net), &show) {
            for prop in &p.props {
                for s in &prop.segments {
                    assert!(s.prop_offset.saturating_add(s.pixel_count) <= prop.pixel_count);
                }
                if let Some(m) = &prop.matrix {
                    assert_eq!(m.pixel_map.len() as u64, m.width as u64 * m.height as u64);
                    assert!(m.pixel_map.iter().all(|&v| v < prop.pixel_count as i32));
                }
            }
            let applied = pixelplus_core::xlights::apply_import(&show, &p, &Default::default());
            assert_eq!(applied.props.len(), p.props.len());
        }
        iterations += 1;
    }
}

#[test]
fn hostile_xml_documents_are_rejected_quickly() {
    let show = Show::default();
    let started = Instant::now();
    // Entity expansion ("billion laughs"): DTDs are allowed because xLights files
    // sometimes carry one, so expansion must be bounded by the XML parser.
    let mut lol = String::from("<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"aaaaaaaaaa\">");
    let mut prev = 'a';
    for c in 'b'..='k' {
        lol.push_str(&format!("<!ENTITY {c} \"&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};&{prev};\">"));
        prev = c;
    }
    lol.push_str("]><xrgb><models><model name=\"&k;\" DisplayAs=\"Single Line\"/></models></xrgb>");
    let _ = import_preview(&lol, None, &show);
    let _ = Networks::parse(&lol, &mut Vec::new());
    // Recursive entity.
    let rec = "<!DOCTYPE x [<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">]><xrgb><models><model name=\"&a;\"/></models></xrgb>";
    assert!(import_preview(rec, None, &show).is_err());
    // Deep nesting.
    let depth = 100_000;
    let deep = format!(
        "<xrgb><models>{}{}</models></xrgb>",
        "<g>".repeat(depth),
        "</g>".repeat(depth)
    );
    // roxmltree recurses per nesting level: this used to overflow the stack and abort.
    assert!(import_preview(&deep, None, &show).is_err());
    assert!(Networks::parse(&deep, &mut Vec::new()).is_err());
    // Huge group membership lists.
    let mut big = String::from("<xrgb><models>");
    for i in 0..3000 {
        big.push_str(&format!(
            r#"<model name="m{i}" DisplayAs="Single Line" parm1="1" parm2="1" StartChannel="{}"/>"#,
            i * 3 + 1
        ));
    }
    big.push_str("</models><modelGroups>");
    let members: Vec<String> = (0..3000).map(|i| format!("m{i}")).collect();
    for g in 0..20 {
        big.push_str(&format!(
            r#"<modelGroup name="g{g}" models="{},g{}"/>"#,
            members.join(","),
            (g + 1) % 20
        ));
    }
    big.push_str("</modelGroups></xrgb>");
    let p = import_preview(&big, None, &show).unwrap();
    assert_eq!(p.groups.len(), 20);
    assert!(started.elapsed() < Duration::from_secs(60));
}
