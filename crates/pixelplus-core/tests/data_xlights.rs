//! End-to-end tests of the xLights importer against realistic fixture files, and of the
//! full data path: import → apply → fseq → NodeMap → .ppseq → power estimate.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use pixelplus_core::fseq::{FseqFile, FseqWriter, FseqWriterOptions};
use pixelplus_core::mapping::{NodeMap, PropMap};
use pixelplus_core::model::{BoardKind, Node, NodeRole, PropKind, Show};
use pixelplus_core::power::{estimate_power, PowerOptions};
use pixelplus_core::ppseq::{write_slice_to, PpseqFile};
use pixelplus_core::xlights::{apply_import, import_preview, ImportPreview};

const RGB: &str = include_str!("../testdata/data_xlights_rgbeffects.xml");
const NET: &str = include_str!("../testdata/data_xlights_networks.xml");

fn node(id: &str, name: &str, board: BoardKind) -> Node {
    Node {
        id: id.into(),
        name: name.into(),
        hostname: id.into(),
        role: NodeRole::Leader,
        board,
        board_rev: None,
        pi_model: None,
        outputs: board.default_outputs(),
        adopted: true,
        last_seen: None,
        notes: None,
    }
}

fn base_show() -> Show {
    let mut s = Show::default();
    s.nodes
        .push(node("lead", "PixelPlus Leader", BoardKind::Difftx));
    s.nodes.push(node("big", "Garage", BoardKind::Difftxlarge));
    s
}

fn preview() -> ImportPreview {
    import_preview(RGB, Some(NET), &base_show()).expect("fixture imports")
}

fn by_name(p: &ImportPreview) -> HashMap<&str, &pixelplus_core::model::Prop> {
    p.props.iter().map(|p| (p.name.as_str(), p)).collect()
}

#[test]
fn imports_props_with_channels_and_kinds() {
    let p = preview();
    let props = by_name(&p);
    let expect = [
        ("Arch 1", 50, 0, PropKind::Arch),
        ("Arch 2", 50, 150, PropKind::Arch),
        ("Matrix", 200, 300, PropKind::Matrix),
        ("Candy Canes", 36, 900, PropKind::Candycane),
        ("Icicles", 60, 1008, PropKind::Icicles),
        ("Old Line", 30, 1200, PropKind::Line),
        ("Spinner", 30, 1290, PropKind::Spinner),
        ("Mega Tree", 400, 3000, PropKind::Tree),
        ("Snowflake", 12, 4530, PropKind::Custom),
        ("Star", 40, 4566, PropKind::Star),
        ("Window", 30, 4686, PropKind::Window),
        ("Face", 3, 7999, PropKind::Custom),
    ];
    assert_eq!(p.props.len(), expect.len(), "{:#?}", p.warnings);
    for (name, px, cs, kind) in expect {
        let prop = props.get(name).unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(prop.pixel_count, px, "{name} pixels");
        assert_eq!(prop.channel_start, cs, "{name} channel start");
        assert_eq!(prop.kind, kind, "{name} kind");
        assert_eq!(prop.xlights_model.as_deref(), Some(name));
    }
    assert_eq!(props["Arch 1"].color.as_deref(), Some("#FF0000"));
    let w = p.warnings.join("\n");
    assert!(w.contains("skipped 'Flood'"), "{w}");
    assert!(w.contains("skipped 'Arch 1 Shadow'"), "{w}");
    assert!(w.contains("'Old Line' has no controller port"), "{w}");
    assert!(w.contains("'Face' has no controller port"), "{w}");
    assert!(w.contains("smart remotes (A, B)"), "{w}");
    assert!(!w.contains("overlapping"), "{w}");
}

#[test]
fn wiring_follows_ports_chains_nulls_and_smart_remotes() {
    let p = preview();
    let props = by_name(&p);
    let seg = |name: &str| props[name].segments.clone();

    let a1 = seg("Arch 1");
    assert_eq!(a1.len(), 1);
    assert_eq!(
        (
            a1[0].node_id.as_str(),
            a1[0].output,
            a1[0].start_pixel,
            a1[0].pixel_count
        ),
        ("PixelPlus Leader", 1, 0, 50)
    );
    let a2 = seg("Arch 2");
    assert_eq!(
        (a2[0].output, a2[0].start_pixel, a2[0].null_pixels),
        (1, 52, 2)
    );

    let spinner = seg("Spinner");
    assert_eq!((spinner[0].output, spinner[0].start_pixel), (2, 0));
    assert!(spinner[0].reverse);

    assert_eq!(seg("Candy Canes")[0].start_pixel, 0);
    let ice = seg("Icicles");
    assert_eq!(
        (ice[0].output, ice[0].start_pixel),
        (4, 37),
        "after 36 cane pixels + 1 end null"
    );

    let tree = seg("Mega Tree");
    assert_eq!(tree.len(), 8);
    for (k, s) in tree.iter().enumerate() {
        assert_eq!(s.node_id, "Garage F16");
        assert_eq!(s.output, k as u32 + 1);
        assert_eq!(
            (s.start_pixel, s.pixel_count, s.prop_offset),
            (0, 50, k as u32 * 50)
        );
    }

    // Remote A (Star) comes before remote B (Snowflake) even though Snowflake's
    // channels are lower.
    let star = seg("Star");
    let flake = seg("Snowflake");
    assert_eq!((star[0].output, star[0].start_pixel), (9, 0));
    assert_eq!((flake[0].output, flake[0].start_pixel), (9, 40));

    let window = seg("Window");
    assert!(window[0].reverse);
    assert_eq!(window[0].output, 10);

    assert!(seg("Old Line").is_empty());

    let ctrls: HashMap<&str, _> = p.controllers.iter().map(|c| (c.name.as_str(), c)).collect();
    let lead = ctrls["PixelPlus Leader"];
    assert_eq!((lead.ports, lead.prop_count), (4, 6));
    assert_eq!(lead.ip.as_deref(), Some("192.168.1.50"));
    assert_eq!(lead.protocol.as_deref(), Some("DDP"));
    assert_eq!(lead.suggested_node_id.as_deref(), Some("lead"));
    let f16 = ctrls["Garage F16"];
    assert_eq!((f16.ports, f16.prop_count), (10, 4));
    assert_eq!(f16.suggested_node_id, None);
}

#[test]
fn matrices_layouts_and_groups() {
    let p = preview();
    let props = by_name(&p);

    let m = props["Matrix"].matrix.as_ref().expect("matrix map");
    assert_eq!((m.width, m.height), (20, 10));
    assert_eq!(m.pixel_map[0], 0, "starts top-left");
    assert_eq!(m.pixel_map[19], 19);
    assert_eq!(m.pixel_map[20 + 19], 20, "second row zig-zags back");

    let flake = props["Snowflake"].matrix.as_ref().unwrap();
    assert_eq!((flake.width, flake.height), (5, 5));
    assert_eq!(flake.pixel_map[2], 0);
    assert_eq!(flake.pixel_map[4 * 5 + 2], 11);
    assert!(
        props["Mega Tree"].matrix.is_none(),
        "360 trees have no flat grid"
    );

    for prop in &p.props {
        let l = prop
            .layout
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no layout", prop.name));
        assert!(
            l.x >= 0.0 && l.y >= 0.0 && l.w > 0.0 && l.h > 0.0,
            "{}: {l:?}",
            prop.name
        );
        let pts = l.points.as_ref().unwrap();
        assert_eq!(pts.len(), prop.pixel_count as usize, "{}", prop.name);
        assert!(pts
            .iter()
            .all(|q| (0.0..=1.0).contains(&q[0]) && (0.0..=1.0).contains(&q[1])));
    }
    // World layout is preserved: the star (WorldPosY=400) is above the icicles (y=300)
    // which are above the arches (y=0) on the y-down canvas.
    let y = |n: &str| props[n].layout.as_ref().unwrap().y;
    assert!(y("Star") < y("Icicles") && y("Icicles") < y("Arch 1"));
    // Arch 1 is left of Arch 2 and its first pixel is on its left end.
    let a1 = props["Arch 1"].layout.as_ref().unwrap();
    assert!(a1.x < props["Arch 2"].layout.as_ref().unwrap().x);
    assert!(a1.points.as_ref().unwrap()[0][0] < 0.1);
    // Candy canes run right-to-left (Dir="R").
    let cc = props["Candy Canes"]
        .layout
        .as_ref()
        .unwrap()
        .points
        .clone()
        .unwrap();
    assert!(cc[0][0] > cc[35][0]);

    let groups: HashMap<&str, _> = p.groups.iter().map(|g| (g.name.as_str(), g)).collect();
    let ids = |names: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|n| props[n].id.clone()).collect();
        v.sort();
        v
    };
    let sorted = |v: &Vec<String>| {
        let mut v = v.clone();
        v.sort();
        v
    };
    assert_eq!(
        sorted(&groups["Arches"].prop_ids),
        ids(&["Arch 1", "Arch 2"])
    );
    assert_eq!(groups["Arches"].color.as_deref(), Some("#00FF00"));
    assert_eq!(
        sorted(&groups["Everything"].prop_ids),
        ids(&["Arch 1", "Arch 2", "Mega Tree"])
    );
    assert_eq!(sorted(&groups["Loop A"].prop_ids), ids(&["Star", "Window"]));
    assert!(props["Arch 1"].group_ids.contains(&groups["Arches"].id));
    let w = p.warnings.join("\n");
    assert!(w.contains("unknown model 'Ghost Model'"), "{w}");
    assert!(w.contains("1 submodel reference"), "{w}");

    // Preview JSON is camelCase.
    let json = serde_json::to_string(&p).unwrap();
    assert!(json.contains("\"suggestedNodeId\":\"lead\""));
    assert!(json.contains("\"channelStart\""));
}

#[test]
fn apply_maps_controllers_and_reimport_keeps_user_edits() {
    let show = base_show();
    let p = preview();
    let mut map = BTreeMap::new();
    map.insert("Garage F16".to_string(), "big".to_string());
    let s1 = apply_import(&show, &p, &map);
    assert_eq!(s1.version, show.version + 1);
    assert_eq!(s1.props.len(), 12);
    assert_eq!(s1.prop_groups.len(), 4);
    let tree = s1.props.iter().find(|p| p.name == "Mega Tree").unwrap();
    assert!(tree.segments.iter().all(|s| s.node_id == "big"));
    let arch = s1.props.iter().find(|p| p.name == "Arch 1").unwrap();
    assert_eq!(
        arch.segments[0].node_id, "lead",
        "leader mapped via suggestion"
    );

    let lead_map = NodeMap::build(&s1, "lead").unwrap();
    assert!(lead_map.warnings.is_empty(), "{:?}", lead_map.warnings);
    assert_eq!(lead_map.pixels_per_output(), &[102, 30, 200, 97]);
    let big_map = NodeMap::build(&s1, "big").unwrap();
    assert!(big_map.warnings.is_empty(), "{:?}", big_map.warnings);
    assert_eq!(
        &big_map.pixels_per_output()[..10],
        &[50, 50, 50, 50, 50, 50, 50, 50, 52, 30]
    );

    // User edits.
    let mut edited = s1.clone();
    let arch_id = arch.id.clone();
    {
        let a = edited.props.iter_mut().find(|p| p.id == arch_id).unwrap();
        a.name = "Driveway Arch".into();
        a.color = Some("#123456".into());
        a.notes = Some("left of driveway".into());
        let l = a.layout.as_mut().unwrap();
        l.x = 5.0;
        l.y = 6.0;
    }
    // Re-import against the edited show (ids are reused) and apply again.
    let p2 = import_preview(RGB, Some(NET), &edited).unwrap();
    assert!(p2.props.iter().any(|p| p.id == arch_id));
    let s2 = apply_import(&edited, &p2, &map);
    assert_eq!(s2.props.len(), 12, "no duplicates on re-import");
    assert_eq!(s2.prop_groups.len(), 4);
    let a = s2.props.iter().find(|p| p.id == arch_id).unwrap();
    assert_eq!(a.name, "Driveway Arch");
    assert_eq!(a.color.as_deref(), Some("#123456"));
    assert_eq!(a.notes.as_deref(), Some("left of driveway"));
    assert_eq!(
        (a.layout.as_ref().unwrap().x, a.layout.as_ref().unwrap().y),
        (5.0, 6.0)
    );
    let g = s2.prop_groups.iter().find(|g| g.name == "Arches").unwrap();
    assert_eq!(g.prop_ids.len(), 2);

    // Unmapped controller: F16 props keep no segments (nothing to keep either).
    let s3 = apply_import(&show, &p, &BTreeMap::new());
    let tree = s3.props.iter().find(|p| p.name == "Mega Tree").unwrap();
    assert!(tree.segments.is_empty());
}

/// Every prop pixel gets a unique colour derived from its absolute pixel index; after
/// routing, each output pixel must carry the colour of the prop pixel wired there.
#[test]
fn full_pipeline_routes_every_pixel() {
    let show = base_show();
    let mut map = BTreeMap::new();
    map.insert("Garage F16".to_string(), "big".to_string());
    let s = apply_import(&show, &preview(), &map);

    let channels = 8100u32;
    let color = |ch_pixel: u32, frame: u32| -> [u8; 3] {
        [(ch_pixel & 0xFF) as u8, (ch_pixel >> 8) as u8, frame as u8]
    };
    let mut opts = FseqWriterOptions::new(channels, 25);
    opts.frames_per_block = 5;
    let mut w = FseqWriter::new(Cursor::new(Vec::new()), opts).unwrap();
    for f in 0..12 {
        let mut frame = vec![0u8; channels as usize];
        for px in 0..channels / 3 {
            frame[(px * 3) as usize..(px * 3 + 3) as usize].copy_from_slice(&color(px, f));
        }
        w.write_frame(&frame).unwrap();
    }
    let bytes = w.finish().unwrap().into_inner();

    for node_id in ["lead", "big"] {
        let nm = NodeMap::build(&s, node_id).unwrap();
        let pm = PropMap::build(&s, node_id).unwrap();
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes.clone())).unwrap();
        let mut src = vec![0u8; fseq.frame_size()];
        let mut out = nm.new_frame();
        fseq.frame(7, &mut src).unwrap();
        nm.render(&src, &mut out);
        let mut checked = 0;
        for prop in s.props.iter().filter(|p| pm.contains(&p.id)) {
            for i in 0..prop.pixel_count {
                if let Some((o, px)) = pm.locate(&prop.id, i) {
                    let got = &out.output(o)[px as usize * 3..px as usize * 3 + 3];
                    assert_eq!(
                        got,
                        color(prop.channel_start / 3 + i, 7),
                        "{} pixel {i}",
                        prop.name
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0);

        // The node's slice reproduces the routed frames exactly.
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes.clone())).unwrap();
        let slice = write_slice_to(&mut fseq, [1; 32], &nm, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();
        let mut pp = PpseqFile::from_reader(Cursor::new(slice)).unwrap();
        let mut from_slice = pp.new_frame();
        pp.frame_into(7, &mut from_slice).unwrap();
        assert_eq!(from_slice, out, "{node_id}");
    }

    // Power estimate covers every wired output.
    let mut fseq = FseqFile::from_reader(Cursor::new(bytes)).unwrap();
    let e = estimate_power(&s, &mut fseq, &PowerOptions::default()).unwrap();
    assert_eq!(e.per_output.len(), 4 + 10);
    assert_eq!(e.frames_sampled, 12);
    assert!(e.per_prop.iter().all(|p| p.peak_amps >= 0.0));
}

// ---------------------------------------------------------------------------
// xLights 2025-format fixture (named attributes, sorted like xLights writes them,
// <Controller> networks with ExtraProperty children, 3D custom model, layered arch,
// tree with exportFirstStrand, multi-string poly line).
// ---------------------------------------------------------------------------

const RGB_2025: &str = include_str!("../testdata/xlights_2025_rgbeffects.xml");
const NET_2025: &str = include_str!("../testdata/xlights_2025_networks.xml");

#[test]
fn xlights_2025_format_imports_with_xlights_channel_math() {
    let show = Show::default();
    let p = import_preview(RGB_2025, Some(NET_2025), &show).expect("2025 fixture imports");
    let props = by_name(&p);
    // (name, pixels, 0-based channel start)
    let expect = [
        ("Big Arch", 100, 0),      // layered arch: NodesPerArch nodes, not layers' sum
        ("Canes", 100, 300),       // >Big Arch:1
        ("Snowflake 3D", 6, 600),  // node 5/6 live on layer 1 only
        ("Star", 60, 618),         // >Snowflake 3D:1
        ("Garage Poly", 150, 798), // @Star:181
        ("Mega Tree", 1600, 6000), // #10.0.0.20:1:1
        ("Matrix", 800, 11100),    // #10.0.0.20:11:1 = 6001 + 10*510
        ("Sphere", 100, 13500),    // >Matrix:1
        ("Window", 60, 16742),
    ];
    for (name, px, ch) in expect {
        let prop = props
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing: {:?}", p.warnings));
        assert_eq!((prop.pixel_count, prop.channel_start), (px, ch), "{name}");
    }
    assert!(!props.contains_key("Flood") && !props.contains_key("Mini Lights"));
    assert!(p
        .warnings
        .iter()
        .any(|w| w.contains("'Window' is not assigned")));

    let segs = |name: &str| {
        let mut v: Vec<(String, u32, u32, u32, u32)> = props[name]
            .segments
            .iter()
            .map(|s| {
                (
                    s.node_id.clone(),
                    s.output,
                    s.start_pixel,
                    s.pixel_count,
                    s.prop_offset,
                )
            })
            .collect();
        v.sort();
        v
    };
    let f = |o, sp, n, off| ("Front PixelPlus".to_string(), o, sp, n, off);
    assert_eq!(segs("Big Arch"), vec![f(1, 0, 100, 0)]);
    assert_eq!(segs("Snowflake 3D"), vec![f(3, 0, 6, 0)]);
    assert_eq!(segs("Star"), vec![f(3, 8, 60, 0)]); // after the snowflake + 2 nulls
    assert_eq!(segs("Garage Poly"), vec![f(4, 0, 75, 0), f(5, 0, 75, 75)]);
    let tree = segs("Mega Tree");
    assert_eq!(tree.len(), 16);
    assert!(tree.iter().enumerate().all(|(i, s)| s.0 == "Tree F48"
        && s.1 == i as u32 + 1
        && s.3 == 100
        && s.4 == 100 * i as u32));
    assert_eq!(
        segs("Matrix").iter().map(|s| s.1).collect::<Vec<_>>(),
        vec![17, 18, 19, 20]
    );
    assert_eq!(
        segs("Sphere")
            .iter()
            .map(|s| (s.1, s.3))
            .collect::<Vec<_>>(),
        vec![(21, 50), (22, 50)]
    );

    // The 3D custom model's map shows both layers side by side.
    let m = props["Snowflake 3D"].matrix.as_ref().unwrap();
    assert_eq!((m.width, m.height), (6, 3));
    // Horizontal matrix starting top-left: pixel 0 is the top-left cell.
    let m = props["Matrix"].matrix.as_ref().unwrap();
    assert_eq!((m.width, m.height), (50, 16));
    assert_eq!(m.pixel_map[0], 0);

    let ctrls: HashMap<&str, (u32, u32, Option<&str>)> = p
        .controllers
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                (c.ports, c.prop_count, c.protocol.as_deref()),
            )
        })
        .collect();
    assert_eq!(ctrls["Front PixelPlus"], (5, 5, Some("DDP")));
    assert_eq!(ctrls["Tree F48"], (22, 3, Some("E131")));
    let g: HashMap<&str, usize> = p
        .groups
        .iter()
        .map(|g| (g.name.as_str(), g.prop_ids.len()))
        .collect();
    assert_eq!(g["Front Yard"], 4);
    assert_eq!(g["Everything"], 7);
}

// ---------------------------------------------------------------------------
// Individual ("Advanced") per-string start channels, inactive models and output
// types xLights does not know.
// ---------------------------------------------------------------------------

const RGB_ADV: &str = include_str!("../testdata/xlights_advanced_rgbeffects.xml");
const NET_ADV: &str = include_str!("../testdata/xlights_advanced_networks.xml");

/// `(prop offset, 0-based byte start, pixels)` runs of a prop, as imported.
fn runs_of(p: &pixelplus_core::model::Prop) -> Option<Vec<(u32, u32, u32)>> {
    p.channel_runs.as_ref().map(|r| {
        r.iter()
            .map(|r| (r.prop_offset, r.channel_start, r.pixel_count))
            .collect()
    })
}

#[test]
fn advanced_start_channels_import_as_channel_runs() {
    let p = import_preview(RGB_ADV, Some(NET_ADV), &Show::default()).expect("imports");
    let props = by_name(&p);
    let w = p.warnings.join("\n");

    // Arches: `!Controller:ch` per arch, arch 2 patched far away, arch 3 right after 1.
    let arches = props["Arches"];
    assert_eq!((arches.pixel_count, arches.channel_start), (30, 0));
    assert_eq!(
        runs_of(arches),
        Some(vec![(0, 0, 10), (10, 300, 10), (20, 30, 10)])
    );
    // Canes: `>Arches:1` follows the arches' *last* channel (331), cane 2 absolute.
    let canes = props["Canes"];
    assert_eq!(canes.channel_start, 330);
    assert_eq!(runs_of(canes), Some(vec![(0, 330, 8), (8, 1999, 8)]));
    // A model chained after an Advanced model starts after its highest channel.
    assert_eq!(props["After Canes"].channel_start, 2023);
    // Icicles number every node from string 1; custom models from the lowest string.
    assert_eq!(
        (props["Icicles"].channel_start, runs_of(props["Icicles"])),
        (2499, None)
    );
    assert_eq!(
        (
            props["Snowflake"].channel_start,
            runs_of(props["Snowflake"])
        ),
        (2699, None)
    );
    // Tree: `#ip:universe:ch`, `!Controller:ch` (contiguous with string 1, so merged),
    // and universe 4 right after universe 2 because xLights drops the unknown output.
    let tree = props["Tree"];
    assert_eq!(tree.channel_start, 3000);
    assert_eq!(
        runs_of(tree),
        Some(vec![(0, 3000, 20), (20, 3510, 10), (30, 4020, 10)])
    );
    assert_eq!(tree.segments.len(), 4);

    // Inactive model: skipped, but still counts for chaining.
    assert!(!props.contains_key("Off Arch"));
    assert!(w.contains("skipped 'Off Arch': it is inactive"), "{w}");
    assert_eq!(props["After Off"].channel_start, 2915);
    assert!(!p.groups[0].prop_ids.is_empty());
    assert!(!w.contains("unknown model 'Off Arch'"), "{w}");

    // Unknown controller / output types are dropped with a warning.
    assert!(
        w.contains("'Old Wireless'") && w.contains("unknown type 'Wireless'"),
        "{w}"
    );
    assert!(w.contains("'FutureProtocol'"), "{w}");
    assert!(!w.contains("not contiguous"), "{w}");
    assert!(!w.contains("cannot resolve"), "{w}");
}

/// Every pixel of every Advanced prop, routed through the NodeMap, the `.ppseq`
/// slice and the prop map, carries the bytes of the channels xLights assigns it.
#[test]
fn advanced_start_channels_round_trip_through_mapping() {
    let mut show = Show::default();
    show.nodes.push(node("porch", "Porch", BoardKind::Difftx));
    show.nodes
        .push(node("yard", "Yard", BoardKind::Difftxlarge));
    let p = import_preview(RGB_ADV, Some(NET_ADV), &show).unwrap();
    let s = apply_import(&show, &p, &BTreeMap::new());
    let get = |n: &str| s.props.iter().find(|p| p.name == n).unwrap();
    assert!(get("Arches").channel_runs.is_some(), "apply keeps runs");

    // xLights channel (0-based byte) of each prop pixel, written out by hand.
    let expected = |name: &str, i: u32| -> u32 {
        match name {
            "Arches" => match i {
                0..=9 => 3 * i,
                10..=19 => 300 + 3 * (i - 10),
                _ => 30 + 3 * (i - 20),
            },
            "Canes" if i < 8 => 330 + 3 * i,
            "Canes" => 1999 + 3 * (i - 8),
            "Tree" => match i {
                0..=19 => 3000 + 3 * i,
                20..=29 => 3510 + 3 * (i - 20),
                _ => 4020 + 3 * (i - 30),
            },
            other => get(other).channel_start + 3 * i,
        }
    };

    let channels = 4530u32;
    let byte = |b: u32, f: u32| ((b * 7 + f * 13) % 251) as u8;
    let mut w = FseqWriter::new(
        Cursor::new(Vec::new()),
        FseqWriterOptions::new(channels, 25),
    )
    .unwrap();
    for f in 0..4 {
        let frame: Vec<u8> = (0..channels).map(|b| byte(b, f)).collect();
        w.write_frame(&frame).unwrap();
    }
    let bytes = w.finish().unwrap().into_inner();

    let mut checked = HashMap::new();
    for node_id in ["porch", "yard"] {
        let nm = NodeMap::build(&s, node_id).unwrap();
        assert!(nm.warnings.is_empty(), "{:?}", nm.warnings);
        let pm = PropMap::build(&s, node_id).unwrap();
        let mut fseq = FseqFile::from_reader(Cursor::new(bytes.clone())).unwrap();
        let slice = write_slice_to(&mut fseq, [2; 32], &nm, Cursor::new(Vec::new()))
            .unwrap()
            .into_inner();
        let mut pp = PpseqFile::from_reader(Cursor::new(slice)).unwrap();
        let mut out = pp.new_frame();
        pp.frame_into(2, &mut out).unwrap();
        for prop in s.props.iter().filter(|p| pm.contains(&p.id)) {
            // Prop-order readback from the output frame, as the preview does.
            let mut rgb = vec![0u8; prop.pixel_count as usize * 3];
            pm.read_prop(&prop.id, &out, &mut rgb);
            for i in 0..prop.pixel_count {
                let Some((o, px)) = pm.locate(&prop.id, i) else {
                    continue;
                };
                let ch = expected(&prop.name, i);
                let want = [byte(ch, 2), byte(ch + 1, 2), byte(ch + 2, 2)];
                assert_eq!(
                    &out.output(o)[px as usize * 3..px as usize * 3 + 3],
                    want,
                    "{} pixel {i} on {node_id}",
                    prop.name
                );
                assert_eq!(&rgb[i as usize * 3..i as usize * 3 + 3], want);
                assert_eq!(prop.channel_of_pixel(i), Some(ch));
                *checked.entry(prop.name.clone()).or_insert(0) += 1;
            }
        }
    }
    assert_eq!(checked["Arches"], 30);
    assert_eq!(checked["Canes"], 16);
    assert_eq!(checked["Tree"], 40);
    assert_eq!(checked["Icicles"], 12);
}
