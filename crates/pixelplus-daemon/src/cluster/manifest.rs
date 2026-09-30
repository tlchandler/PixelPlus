//! Node manifests (ARCHITECTURE §7.2): everything a follower needs to know,
//! computed by the leader from the show, and the follower-local [`Show`] built
//! from it so the player works identically on leader and followers.

use super::slices;
use pixelplus_core::effects::stamp_world_bounds;
use pixelplus_core::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// A sequence as a follower sees it: its node slice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSequence {
    pub id: String,
    pub name: String,
    /// Slice key (also the HTTP ETag): changes whenever the slice's bytes would.
    pub hash: String,
    /// sha256 of the source `.fseq`.
    #[serde(default)]
    pub source_hash: String,
    pub duration_ms: u64,
    pub frame_ms: u32,
    /// Bytes per slice frame (sum of this node's pixels × 3).
    #[serde(default)]
    pub frame_bytes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_id: Option<String>,
}

/// Show-wide settings a follower needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSettings {
    #[serde(default)]
    pub location: Location,
    #[serde(default)]
    pub oled: OledSettings,
    /// Pixel output options (latch alignment).
    #[serde(default)]
    pub output: OutputSettings,
    /// Props kept dark by the active season profile (F8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_prop_ids: Vec<String>,
}

/// What the leader tells one follower (`GET /cluster/manifest/:nodeId`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NodeManifest {
    pub show_version: u64,
    pub leader_id: String,
    pub show_name: String,
    /// The follower's own node (outputs, name, board).
    pub node: Node,
    /// Props with at least one segment on this node; segments filtered to it.
    pub props: Vec<Prop>,
    /// Groups, restricted to the props above.
    #[serde(default)]
    pub prop_groups: Vec<PropGroup>,
    /// Receivers plugged into this node.
    #[serde(default)]
    pub receivers: Vec<Receiver>,
    /// All looks, stamped with the display-wide world bounds.
    pub effects: Vec<EffectPreset>,
    pub sequences: Vec<ManifestSequence>,
    #[serde(default)]
    pub settings: ManifestSettings,
    /// Hash of this node's pixel routing; slices are keyed by it.
    #[serde(default)]
    pub mapping_hash: String,
    /// Power limiter budget for this node (F12; populated by WS5 from
    /// `power::node_budget`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<NodePowerBudget>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("{0} is not part of this show")]
    UnknownNode(String),
    #[error("{0} is the show leader, not a follower")]
    NotFollower(String),
}

/// Build the manifest for `node_id` (leader side).
///
/// Sequences without a readable `.fseq` on the leader are left out (the follower
/// could not get a slice for them anyway).
pub fn build(
    show: &Show,
    leader_id: &str,
    node_id: &str,
    data_dir: &Path,
) -> Result<NodeManifest, ManifestError> {
    let node = show
        .node(node_id)
        .ok_or_else(|| ManifestError::UnknownNode(node_id.to_string()))?;
    if node.role == NodeRole::Leader {
        return Err(ManifestError::NotFollower(node.name.clone()));
    }
    let props: Vec<Prop> = show
        .props
        .iter()
        .filter(|p| p.segments.iter().any(|s| s.node_id == node_id))
        .map(|p| {
            let mut p = p.clone();
            p.segments.retain(|s| s.node_id == node_id);
            p
        })
        .collect();
    let prop_ids: BTreeSet<&str> = props.iter().map(|p| p.id.as_str()).collect();
    let prop_groups = show
        .prop_groups
        .iter()
        .map(|g| {
            let mut g = g.clone();
            g.prop_ids.retain(|id| prop_ids.contains(id.as_str()));
            g
        })
        .collect();
    let effects = show
        .effects
        .iter()
        .map(|e| {
            let mut e = e.clone();
            stamp_world_bounds(&mut e, &show.props);
            e
        })
        .collect();
    let map = pixelplus_core::mapping::NodeMap::build(show, node_id)
        .map_err(|_| ManifestError::UnknownNode(node_id.to_string()))?;
    let mapping_hash = slices::mapping_hash(&map);
    let frame_bytes = (map.total_pixels() * 3) as u32;
    let sequences = show
        .sequences
        .iter()
        .filter_map(|seq| {
            let fseq = data_dir.join(&seq.file);
            let source = slices::source_id(seq, &fseq)?;
            Some(ManifestSequence {
                id: seq.id.clone(),
                name: seq.name.clone(),
                hash: slices::slice_key(&source, &mapping_hash),
                source_hash: seq.hash.clone(),
                duration_ms: seq.duration_ms,
                frame_ms: seq.frame_ms,
                frame_bytes,
                media_id: seq.media_id.clone(),
            })
        })
        .collect();
    let mut node = node.clone();
    node.last_seen = None;
    Ok(NodeManifest {
        show_version: show.version,
        leader_id: leader_id.to_string(),
        show_name: show.name.clone(),
        node,
        props,
        prop_groups,
        receivers: show
            .receivers
            .iter()
            .filter(|r| r.node_id == node_id)
            .cloned()
            .collect(),
        effects,
        sequences,
        settings: ManifestSettings {
            location: show.schedule.location.clone(),
            oled: show.settings.oled.clone(),
            output: show.settings.output.clone(),
            // Populated from the active season profile by WS5/WS6 (F8).
            disabled_prop_ids: Vec::new(),
        },
        mapping_hash,
        // Populated from `power::node_budget` by WS5/WS3 (F12).
        power: None,
    })
}

/// Relative path (under the data dir) of a follower's slice for `seq_id`.
pub fn follower_slice_file(seq_id: &str) -> String {
    format!("sequences/{seq_id}.ppseq")
}

/// Build the follower-local show from a manifest.
///
/// `available` holds ids of sequences whose slice is on disk and verified; only
/// those are listed so the player never tries to open a missing file.
/// `current` supplies what stays local: the password and the version counter.
pub fn follower_show(
    manifest: &NodeManifest,
    available: &HashMap<String, bool>,
    current: &Show,
) -> Show {
    let mut node = manifest.node.clone();
    node.role = NodeRole::Follower;
    node.adopted = true;
    let sequences = manifest
        .sequences
        .iter()
        .filter(|s| available.get(&s.id).copied().unwrap_or(false))
        .map(|s| Sequence {
            generated: Default::default(),
            tags: Default::default(),
            id: s.id.clone(),
            name: s.name.clone(),
            file: follower_slice_file(&s.id),
            duration_ms: s.duration_ms,
            frame_ms: s.frame_ms,
            channel_count: s.frame_bytes,
            media_id: s.media_id.clone(),
            xlights_name: None,
            thumbnail: None,
            hash: s.hash.clone(),
        })
        .collect();
    Show {
        version: current.version,
        name: manifest.show_name.clone(),
        nodes: vec![node],
        receivers: manifest.receivers.clone(),
        props: manifest.props.clone(),
        prop_groups: manifest.prop_groups.clone(),
        sequences,
        effects: manifest.effects.clone(),
        schedule: Schedule {
            // Followers never schedule on their own: the leader drives them.
            enabled: false,
            location: manifest.settings.location.clone(),
            ..Schedule::default()
        },
        settings: ShowSettings {
            oled: manifest.settings.oled.clone(),
            output: manifest.settings.output.clone(),
            security: current.settings.security.clone(),
            ..ShowSettings::default()
        },
        ..Show::default()
    }
}

/// The show a follower shows while it has no leader (dark, own node only).
pub fn standalone_show(node: Node, current: &Show) -> Show {
    Show {
        version: current.version,
        name: current.name.clone(),
        nodes: vec![node],
        schedule: Schedule {
            enabled: false,
            ..Schedule::default()
        },
        settings: ShowSettings {
            security: current.settings.security.clone(),
            ..ShowSettings::default()
        },
        ..Show::default()
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
pub(crate) mod tests {
    use super::*;

    pub fn node(id: &str, role: NodeRole, board: BoardKind) -> Node {
        Node {
            hardware_history: Default::default(),
            serial: Default::default(),
            id: id.into(),
            name: format!("Node {id}"),
            hostname: format!("pp-{id}"),
            role,
            board,
            board_rev: None,
            pi_model: None,
            outputs: board.default_outputs(),
            adopted: true,
            last_seen: None,
            notes: None,
        }
    }

    pub fn seg(node: &str, output: u32, start: u32, count: u32, offset: u32) -> PropSegment {
        PropSegment {
            node_id: node.into(),
            output,
            start_pixel: start,
            pixel_count: count,
            prop_offset: offset,
            reverse: false,
            null_pixels: 0,
        }
    }

    pub fn prop(id: &str, pixels: u32, channel_start: u32, segments: Vec<PropSegment>) -> Prop {
        Prop {
            suspect_pixels: Default::default(),
            id: id.into(),
            name: format!("Prop {id}"),
            kind: PropKind::Line,
            pixel_count: pixels,
            xlights_model: None,
            channel_start,
            channels_per_pixel: 3,
            channel_runs: None,
            segments,
            group_ids: vec![],
            layout: Some(PropLayout {
                source: Default::default(),
                x: channel_start as f32,
                y: 0.0,
                w: 100.0,
                h: 10.0,
                rotation: 0.0,
                points: None,
            }),
            matrix: None,
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    pub fn three_node_show() -> Show {
        let mut s = Show::default();
        s.nodes = vec![
            node("leader", NodeRole::Leader, BoardKind::Difftx),
            node("f1", NodeRole::Follower, BoardKind::Difftx),
            node("f2", NodeRole::Follower, BoardKind::Diffsmart),
        ];
        s.props = vec![
            prop("a", 10, 0, vec![seg("leader", 1, 0, 10, 0)]),
            prop("b", 20, 30, vec![seg("f1", 2, 0, 20, 0)]),
            // Split across two followers.
            prop(
                "c",
                10,
                90,
                vec![seg("f1", 1, 0, 4, 0), seg("f2", 3, 5, 6, 4)],
            ),
        ];
        s.prop_groups = vec![PropGroup {
            id: "g".into(),
            name: "All".into(),
            prop_ids: vec!["a".into(), "b".into(), "c".into()],
            color: None,
        }];
        s.receivers = vec![Receiver {
            main_fuse_amps: Default::default(),
            id: "r1".into(),
            name: "Garage".into(),
            kind: ReceiverKind::Diffrx,
            node_id: "f1".into(),
            jack: 1,
            location: None,
            fuse_amps: None,
            notes: None,
        }];
        s.effects = vec![EffectPreset {
            id: "e".into(),
            name: "Wash".into(),
            effect: EffectKind::Colorwash,
            params: Default::default(),
            target: Target {
                all: true,
                ..Default::default()
            },
        }];
        s
    }

    #[test]
    fn manifest_only_contains_the_nodes_props_and_segments() {
        let show = three_node_show();
        let dir = std::env::temp_dir();
        let m = build(&show, "leader", "f1", &dir).unwrap();
        let ids: Vec<_> = m.props.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
        assert!(m
            .props
            .iter()
            .all(|p| p.segments.iter().all(|s| s.node_id == "f1")));
        assert_eq!(m.props[1].segments.len(), 1);
        assert_eq!(m.prop_groups[0].prop_ids, ["b", "c"]);
        assert_eq!(m.receivers.len(), 1);
        assert_eq!(m.node.id, "f1");
        assert_eq!(m.leader_id, "leader");

        let m2 = build(&show, "leader", "f2", &dir).unwrap();
        assert_eq!(m2.props.len(), 1);
        assert_eq!(m2.props[0].segments, vec![seg("f2", 3, 5, 6, 4)]);
        assert!(m2.receivers.is_empty());
        assert_ne!(m.mapping_hash, m2.mapping_hash);
    }

    #[test]
    fn manifest_effects_are_stamped_with_world_bounds() {
        let show = three_node_show();
        let m = build(&show, "leader", "f2", &std::env::temp_dir()).unwrap();
        let mut expected = show.effects[0].clone();
        stamp_world_bounds(&mut expected, &show.props);
        assert_eq!(m.effects[0], expected);
        assert_ne!(
            m.effects[0], show.effects[0],
            "stamping adds the world bounds"
        );
    }

    #[test]
    fn manifest_rejects_leader_and_unknown_nodes() {
        let show = three_node_show();
        let dir = std::env::temp_dir();
        assert!(matches!(
            build(&show, "leader", "leader", &dir),
            Err(ManifestError::NotFollower(_))
        ));
        assert!(matches!(
            build(&show, "leader", "zz", &dir),
            Err(ManifestError::UnknownNode(_))
        ));
    }

    #[test]
    fn follower_show_lists_only_available_sequences_and_keeps_password() {
        let show = three_node_show();
        let mut m = build(&show, "leader", "f1", &std::env::temp_dir()).unwrap();
        m.sequences = vec![
            ManifestSequence {
                id: "s1".into(),
                name: "Song".into(),
                hash: "k1".into(),
                source_hash: "h".into(),
                duration_ms: 1000,
                frame_ms: 25,
                frame_bytes: 72,
                media_id: None,
            },
            ManifestSequence {
                id: "s2".into(),
                name: "Other".into(),
                hash: "k2".into(),
                source_hash: "h2".into(),
                duration_ms: 1000,
                frame_ms: 25,
                frame_bytes: 72,
                media_id: None,
            },
        ];
        let mut current = Show::default();
        current.version = 42;
        current.settings.security.password_hash = Some("hash".into());
        let avail = HashMap::from([("s1".to_string(), true)]);
        let fs = follower_show(&m, &avail, &current);
        assert_eq!(fs.version, 42);
        assert_eq!(fs.nodes.len(), 1);
        assert_eq!(fs.nodes[0].role, NodeRole::Follower);
        assert_eq!(fs.sequences.len(), 1);
        assert_eq!(fs.sequences[0].file, "sequences/s1.ppseq");
        assert_eq!(fs.settings.security.password_hash.as_deref(), Some("hash"));
        assert!(!fs.schedule.enabled);
        assert_eq!(fs.props.len(), 2);
        // The follower's own mapping works with the filtered props.
        let map = pixelplus_core::mapping::NodeMap::build(&fs, "f1").unwrap();
        assert_eq!(map.pixels_per_output()[..2], [4, 20]);
    }
}
