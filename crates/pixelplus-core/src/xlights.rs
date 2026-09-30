//! xLights layout import (ARCHITECTURE §5).
//!
//! [`import_preview`] reads `xlights_rgbeffects.xml` (and optionally
//! `xlights_networks.xml`) and proposes props, their wiring and groups without touching
//! the show; [`apply_import`] merges a (possibly user-edited) preview into a show.
//!
//! # What is imported
//!
//! * Every pixel model (`<model>`) becomes a [`Prop`]: pixel count from the model's
//!   `DisplayAs` parameters, `channelStart` from its `StartChannel` (all xLights forms:
//!   absolute, `!Controller:ch`, `@Model:ch`, `>Model:ch`, `#universe:ch`,
//!   `#ip:universe:ch`), kind, preview geometry and — for matrices, custom models and
//!   flat trees — a [`MatrixInfo`](crate::model::MatrixInfo) grid map.
//! * `ControllerConnection` ports become [`PropSegment`]s: model port → node output,
//!   models chained on one port get consecutive pixels in start-channel order (after
//!   start/end null pixels), multi-string models continue on the following ports, and
//!   smart-remote chains (A, B, C…) on a port are laid out consecutively on that output.
//! * `<modelGroup>`s become [`PropGroup`]s (nested groups are flattened; submodel
//!   references are skipped).
//!
//! Models that are not RGB pixel strings (channel blocks, DMX fixtures, single-colour or
//! RGBW strings, shadow models, images, labels) are skipped with a warning; they still
//! count for start-channel chaining.
//!
//! # Controller placeholders
//!
//! In an [`ImportPreview`] the xLights→PixelPlus controller mapping is not decided yet,
//! so each proposed segment's `nodeId` holds the **xLights controller name**.
//! [`apply_import`] replaces it with the node chosen in `controllerMap` (falling back to
//! the controller's `suggestedNodeId`); segments whose controller is not mapped to an
//! existing node are dropped.

mod geometry;
pub mod networks;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

use crate::layout;
use crate::model::{new_id, Prop, PropGroup, PropLayout, PropSegment, Show};
use geometry::{shape, strtol, Attrs, Placement, Shape};
pub use networks::{NetController, NetOutput, Networks};

/// Errors that prevent an import altogether (individual model problems are warnings).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImportError {
    /// The rgbeffects file is not well-formed XML.
    #[error("xlights_rgbeffects.xml is not valid XML: {0}")]
    RgbEffectsXml(String),
    /// The networks file is not well-formed XML.
    #[error("xlights_networks.xml is not valid XML: {0}")]
    NetworksXml(String),
    /// The file parsed but has no `<models>` section.
    #[error("this does not look like an xlights_rgbeffects.xml file (no <models> section)")]
    NotRgbEffects,
}

/// An xLights controller that props are wired to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportController {
    /// xLights controller name (key of `controllerMap`).
    pub name: String,
    /// Highest pixel port used by any imported prop.
    pub ports: u32,
    /// Existing node that looks like this controller (same name or hostname).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_node_id: Option<String>,
    /// Controller IP address from xlights_networks.xml.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    /// Controller protocol (DDP, E131, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Number of props with segments on this controller.
    #[serde(default)]
    pub prop_count: u32,
}

/// Result of [`import_preview`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    /// Proposed props (segment `nodeId` = xLights controller name, see module docs).
    pub props: Vec<Prop>,
    /// Controllers referenced by the props.
    pub controllers: Vec<ImportController>,
    /// Groups from xLights model groups.
    pub groups: Vec<PropGroup>,
    /// Everything the user should know about (skipped models, unresolved channels, ...).
    pub warnings: Vec<String>,
}

const NO_CONTROLLER: &[&str] = &["No Controller", "Use Start Channel", "Multiple"];
/// Margin kept around the imported layout on the canvas.
const CANVAS_MARGIN: f32 = 20.0;

#[derive(Debug, Clone, Default)]
struct Conn {
    port: u32,
    protocol: Option<String>,
    smart_remote: u32,
    sr_max_cascade: u32,
    sr_cascade_on_port: bool,
    nulls: u32,
    end_nulls: u32,
    reverse: bool,
}

struct XModel<'a, 'i> {
    name: String,
    node: Node<'a, 'i>,
    shape: Shape,
    conn: Conn,
}

#[derive(Clone, Copy, PartialEq)]
enum Resolve {
    Pending,
    Visiting,
    Done(Option<u32>),
}

struct Resolver<'m, 'a, 'i> {
    models: &'m [XModel<'a, 'i>],
    by_name: HashMap<&'m str, usize>,
    nets: Option<&'m Networks>,
    state: Vec<Resolve>,
    warnings: Vec<String>,
}

impl Resolver<'_, '_, '_> {
    /// 1-based absolute start channel of model `i`.
    ///
    /// Resolution is iterative: xLights' default for a new model is `>Previous:1`, so
    /// real layouts contain chains thousands of models long, which must not grow the
    /// call stack.
    fn start(&mut self, i: usize) -> Option<u32> {
        match self.state[i] {
            Resolve::Done(v) => return v,
            Resolve::Visiting => {
                self.warnings.push(format!(
                    "'{}': start channel refers back to itself through other models",
                    self.models[i].name
                ));
                return None;
            }
            Resolve::Pending => {}
        }
        self.state[i] = Resolve::Visiting;
        let mut stack = vec![i];
        while let Some(&top) = stack.last() {
            if let Some(dep) = self.dependency(top) {
                if self.state[dep] == Resolve::Pending {
                    self.state[dep] = Resolve::Visiting;
                    stack.push(dep);
                    continue;
                }
            }
            let expr = self.start_expr(top);
            let v = self.eval(top, &expr);
            self.state[top] = Resolve::Done(v);
            stack.pop();
        }
        match self.state[i] {
            Resolve::Done(v) => v,
            _ => None,
        }
    }

    fn start_expr(&self, i: usize) -> String {
        Attrs(self.models[i].node)
            .str("StartChannel")
            .unwrap_or("1")
            .to_string()
    }

    /// The model whose start channel model `i`'s start channel is relative to.
    fn dependency(&self, i: usize) -> Option<usize> {
        let expr = Attrs(self.models[i].node).str("StartChannel")?;
        let (head, _) = expr.trim().split_once(':')?;
        let other = head.strip_prefix(['@', '<', '>'])?.trim();
        self.by_name.get(other).copied().filter(|&j| j != i)
    }

    fn eval(&mut self, self_idx: usize, expr: &str) -> Option<u32> {
        let name = self.models[self_idx].name.clone();
        let expr = expr.trim();
        let fail = |w: &mut Vec<String>, why: String| {
            w.push(format!(
                "'{name}': cannot resolve start channel '{expr}': {why}"
            ));
            None
        };
        let Some((head, rest)) = expr.split_once(':') else {
            let v = strtol(expr);
            return match u32::try_from(v) {
                Ok(v) if v >= 1 => Some(v),
                _ => fail(&mut self.warnings, "not a valid channel number".into()),
            };
        };
        let ch = strtol(rest);
        let result: i64 = match head.chars().next() {
            Some(c @ ('@' | '<' | '>')) => {
                let other = head[1..].trim();
                let Some(&j) = self.by_name.get(other) else {
                    return fail(&mut self.warnings, format!("no model named '{other}'"));
                };
                if j == self_idx {
                    return fail(&mut self.warnings, "refers to itself".into());
                }
                let Some(s) = self.start(j) else {
                    return fail(
                        &mut self.warnings,
                        format!("'{other}' has no start channel"),
                    );
                };
                if c == '@' {
                    (s as i64 - 1).saturating_add(ch)
                } else {
                    let chans = self.models[j].shape.channels() as i64;
                    (s as i64 + chans - 1).saturating_add(ch)
                }
            }
            Some('!') => {
                let Some(nets) = self.nets else {
                    return fail(
                        &mut self.warnings,
                        "controller-relative start channels need xlights_networks.xml".into(),
                    );
                };
                let cname = head[1..].trim();
                match nets.controller(cname) {
                    Some(c) => (c.start as i64 - 1).saturating_add(ch),
                    None => {
                        return fail(&mut self.warnings, format!("no controller named '{cname}'"))
                    }
                }
            }
            Some('#') => {
                let Some(nets) = self.nets else {
                    return fail(
                        &mut self.warnings,
                        "universe start channels need xlights_networks.xml".into(),
                    );
                };
                let parts: Vec<&str> = expr[1..].split(':').map(str::trim).collect();
                let found = match parts.as_slice() {
                    [u, c] => {
                        nets.universe_channel(None, to_u32(strtol(u)), to_u32(strtol(c)).max(1))
                    }
                    [ip, u, c] => {
                        nets.universe_channel(Some(ip), to_u32(strtol(u)), to_u32(strtol(c)).max(1))
                    }
                    _ => None,
                };
                match found {
                    Some(v) => v as i64,
                    None => return fail(&mut self.warnings, "universe not found".into()),
                }
            }
            _ => ch,
        };
        if result < 1 {
            return fail(&mut self.warnings, "resolves to a channel before 1".into());
        }
        match u32::try_from(result) {
            Ok(v) => Some(v),
            Err(_) => fail(&mut self.warnings, "channel number is too large".into()),
        }
    }
}

/// Deepest element nesting accepted in xLights files (real files nest about 5 deep).
/// The XML parser recurses per level, so a hostile, deeply nested upload would
/// otherwise overflow the stack and abort the whole daemon.
pub(crate) const MAX_XML_DEPTH: usize = 256;

/// Parse an xLights XML file (DTDs allowed, nesting bounded by [`MAX_XML_DEPTH`]).
pub(crate) fn parse_xml(text: &str) -> Result<Document<'_>, XmlError> {
    if xml_depth_exceeds(text.as_bytes(), MAX_XML_DEPTH) {
        return Err(XmlError::TooDeep);
    }
    Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(XmlError::Xml)
}

/// Why an xLights XML file could not be parsed.
#[derive(Debug)]
pub(crate) enum XmlError {
    Xml(roxmltree::Error),
    TooDeep,
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XmlError::Xml(e) => e.fmt(f),
            XmlError::TooDeep => write!(
                f,
                "elements are nested more than {MAX_XML_DEPTH} levels deep"
            ),
        }
    }
}

/// Conservative linear scan: does element nesting exceed `limit`? Skips comments,
/// CDATA, processing instructions and declarations, and honours quoted attribute
/// values, so it never undercounts the depth the XML parser will see.
fn xml_depth_exceeds(b: &[u8], limit: usize) -> bool {
    let find = |from: usize, pat: &[u8]| -> usize {
        b[from.min(b.len())..]
            .windows(pat.len())
            .position(|w| w == pat)
            .map_or(b.len(), |p| from + p + pat.len())
    };
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = find(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = find(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = find(i + 2, b"?>");
        } else if rest.starts_with(b"<!") {
            // DOCTYPE and its internal subset: entries are `<!...>` declarations.
            i = find(i + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(i + 2, b">");
        } else {
            // Start tag: find its end, honouring quotes; `/>` closes it immediately.
            let mut j = i + 1;
            let mut quote = 0u8;
            let mut self_closing = false;
            while j < b.len() {
                let c = b[j];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    self_closing = b[j - 1] == b'/';
                    break;
                }
                j += 1;
            }
            if !self_closing {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            i = j + 1;
        }
    }
    false
}

/// Clamp a parsed integer into `u32`.
fn to_u32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

fn parse_conn(model: Node) -> Conn {
    let Some(cc) = model
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "ControllerConnection")
    else {
        return Conn::default();
    };
    let a = Attrs(cc);
    let u = |v: Option<i64>| v.unwrap_or(0).clamp(0, 1_000_000) as u32;
    Conn {
        port: u(a.int("Port")),
        protocol: a.str("Protocol").map(str::to_string),
        smart_remote: u(a.int("SmartRemote")),
        sr_max_cascade: u(a.int("SRMaxCascade")).max(1),
        sr_cascade_on_port: a
            .str("SRCascadeOnPort")
            .is_some_and(|s| s.eq_ignore_ascii_case("true")),
        nulls: u(a.int("nullNodes")),
        end_nulls: u(a.int("endNullNodes")),
        reverse: a.int("Reverse").or_else(|| a.int("reverse")).unwrap_or(0) != 0,
    }
}

/// Port and smart remote of 0-based string `s` (xLights `GetPortSR`).
fn port_sr(conn: &Conn, s: u32) -> (u32, u32) {
    const PORTS_PER_SMART_REMOTE: u32 = 4;
    let sr = conn.smart_remote;
    if conn.port == 0 || s == 0 {
        return (conn.port, sr);
    }
    if sr == 0 {
        return (conn.port + s, 0);
    }
    let max = conn.sr_max_cascade.max(1);
    if conn.sr_cascade_on_port {
        return (conn.port + s / max, sr + s % max);
    }
    let (mut p, mut r) = (conn.port, sr);
    for _ in 0..s {
        let np = p + 1;
        if (np - 1) / PORTS_PER_SMART_REMOTE != (p - 1) / PORTS_PER_SMART_REMOTE {
            let nr = r + 1;
            if nr - sr >= max {
                r = sr;
                p = np;
            } else {
                r = nr;
                p = ((p - 1) / PORTS_PER_SMART_REMOTE) * PORTS_PER_SMART_REMOTE + 1;
            }
        } else {
            p = np;
        }
    }
    (p, r)
}

fn is_pixel_protocol(p: Option<&str>) -> bool {
    let Some(p) = p else {
        return true;
    };
    let p = p.to_ascii_lowercase();
    ![
        "dmx",
        "pixelnet",
        "renard",
        "lor",
        "opendmx",
        "genericserial",
        "pwm",
        "virtual matrix",
        "led panel matrix",
    ]
    .iter()
    .any(|s| p.starts_with(s))
}

fn smart_letter(sr: u32) -> String {
    if sr == 0 {
        String::new()
    } else {
        char::from_u32('A' as u32 + (sr - 1).min(25))
            .map(String::from)
            .unwrap_or_default()
    }
}

fn norm_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Existing node whose name or hostname matches an xLights controller name.
pub fn suggest_node(show: &Show, controller: &str, ip: Option<&str>) -> Option<String> {
    let want = norm_name(controller);
    show.nodes
        .iter()
        .find(|n| {
            let host = n.hostname.trim_end_matches(".local");
            (!want.is_empty() && (norm_name(&n.name) == want || norm_name(host) == want))
                || ip.is_some_and(|ip| n.hostname == ip)
        })
        .map(|n| n.id.clone())
}

struct Entry {
    prop: usize,
    sr: u32,
    start_ch: u64,
    offset: u32,
    count: u32,
    nulls: u32,
    end_nulls: u32,
    reverse: bool,
}

/// Build an import preview from xLights files. `show` is only read (to reuse ids of
/// previously imported props and suggest node mappings).
pub fn import_preview(
    rgbeffects_xml: &str,
    networks_xml: Option<&str>,
    show: &Show,
) -> Result<ImportPreview, ImportError> {
    let mut warnings = Vec::new();
    let nets = match networks_xml {
        Some(x) if !x.trim().is_empty() => Some(
            Networks::parse(x, &mut warnings)
                .map_err(|e| ImportError::NetworksXml(e.to_string()))?,
        ),
        _ => None,
    };
    let doc = parse_xml(rgbeffects_xml).map_err(|e| ImportError::RgbEffectsXml(e.to_string()))?;
    let root = doc.root_element();
    let models_el = root
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "models")
        .ok_or(ImportError::NotRgbEffects)?;

    // ---- parse models --------------------------------------------------------------
    let mut models: Vec<XModel> = Vec::new();
    let mut seen = HashSet::new();
    for m in models_el
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "model")
    {
        let a = Attrs(m);
        let Some(name) = a.str("name") else {
            warnings.push("skipped a model without a name".into());
            continue;
        };
        if a.str("DisplayAs") == Some("ModelGroup") {
            continue;
        }
        if !seen.insert(name.to_string()) {
            warnings.push(format!(
                "duplicate model name '{name}': only the first one is imported"
            ));
            continue;
        }
        models.push(XModel {
            name: name.to_string(),
            node: m,
            shape: shape(a),
            conn: parse_conn(m),
        });
    }

    // ---- resolve start channels ------------------------------------------------------
    let mut resolver = Resolver {
        by_name: models
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name.as_str(), i))
            .collect(),
        models: &models,
        nets: nets.as_ref(),
        state: vec![Resolve::Pending; models.len()],
        warnings: Vec::new(),
    };
    let starts: Vec<Option<u32>> = (0..models.len()).map(|i| resolver.start(i)).collect();
    // Individual per-string start channels ("Advanced").
    let mut string_starts: Vec<Option<Vec<Option<u32>>>> = Vec::with_capacity(models.len());
    for (i, m) in models.iter().enumerate() {
        let a = Attrs(m.node);
        if a.int("Advanced").unwrap_or(0) != 0 && m.shape.skip.is_none() {
            let n = m.shape.physical_strings;
            let v = (0..n)
                .map(|s| match a.str(&format!("String{}", s + 1)) {
                    Some(expr) => resolver.eval(i, expr),
                    None => None,
                })
                .collect();
            string_starts.push(Some(v));
        } else {
            string_starts.push(None);
        }
    }
    warnings.append(&mut resolver.warnings);

    // ---- props --------------------------------------------------------------------
    let existing: HashMap<&str, &Prop> = show
        .props
        .iter()
        .filter_map(|p| p.xlights_model.as_deref().map(|n| (n, p)))
        .collect();
    let mut props: Vec<Prop> = Vec::new();
    let mut prop_model: Vec<usize> = Vec::new();
    let mut prop_by_name: HashMap<String, usize> = HashMap::new();
    let mut any_position = false;
    for (i, m) in models.iter().enumerate() {
        let a = Attrs(m.node);
        if let Some(reason) = &m.shape.skip {
            warnings.push(format!("skipped '{}': {reason}", m.name));
            continue;
        }
        if let Some(sh) = a.str("ShadowModelFor") {
            warnings.push(format!(
                "skipped '{}': it is a shadow of '{sh}' and shares its channels",
                m.name
            ));
            continue;
        }
        let Some(start) = starts[i] else {
            continue; // warning already recorded
        };
        if m.shape.nodes == 0 {
            warnings.push(format!("skipped '{}': it has no pixels", m.name));
            continue;
        }
        let has_pos = a.str("WorldPosX").is_some() || m.shape.placement == Placement::World;
        any_position |= has_pos;
        let layout = if has_pos {
            make_layout(&m.shape, a)
        } else {
            None
        };
        let id = existing
            .get(m.name.as_str())
            .map(|p| p.id.clone())
            .unwrap_or_else(new_id);
        let color = a
            .str("TagColour")
            .filter(|c| !c.eq_ignore_ascii_case("#000000") && c.starts_with('#'))
            .map(str::to_string);
        prop_by_name.insert(m.name.clone(), props.len());
        prop_model.push(i);
        props.push(Prop {
            id,
            name: m.name.clone(),
            kind: m.shape.kind,
            pixel_count: m.shape.nodes,
            xlights_model: Some(m.name.clone()),
            channel_start: start - 1,
            channels_per_pixel: 3,
            segments: Vec::new(),
            group_ids: Vec::new(),
            layout,
            matrix: m.shape.matrix.clone(),
            color,
            max_milliamps_per_pixel: None,
            notes: a.str("Description").map(str::to_string),
        });
    }
    if !any_position && !props.is_empty() {
        warnings.push("the layout has no model positions; props were auto-arranged".into());
    }

    // ---- controllers & wiring -----------------------------------------------------------
    let mut ports: BTreeMap<(String, u32), Vec<Entry>> = BTreeMap::new();
    let mut controllers: BTreeMap<String, (u32, BTreeSet<usize>)> = BTreeMap::new();
    for (pi, &mi) in prop_model.iter().enumerate() {
        let m = &models[mi];
        let a = Attrs(m.node);
        let start = starts[mi].unwrap_or(1);
        let ctrl = a
            .str("Controller")
            .filter(|c| !NO_CONTROLLER.iter().any(|n| n.eq_ignore_ascii_case(c)))
            .map(str::to_string)
            .or_else(|| {
                a.str("StartChannel")
                    .and_then(|s| s.strip_prefix('!'))
                    .and_then(|s| s.split(':').next())
                    .map(|s| s.trim().to_string())
            })
            .or_else(|| {
                nets.as_ref()
                    .and_then(|n| n.controller_for_channel(start))
                    .map(|c| c.name.clone())
            });
        let Some(ctrl) = ctrl else {
            warnings.push(format!(
                "'{}' is not assigned to a controller; wire it in PixelPlus",
                m.name
            ));
            continue;
        };
        let ctrl = nets
            .as_ref()
            .and_then(|n| n.controller(&ctrl))
            .map(|c| c.name.clone())
            .unwrap_or(ctrl);
        if let Some(c) = nets.as_ref().and_then(|n| n.controller(&ctrl)) {
            let end = start as u64 + m.shape.channels() - 1;
            if (start < c.start || end > c.end() as u64) && c.channels > 0 {
                warnings.push(format!(
                    "'{}' (channels {start}-{end}) extends outside controller '{}' (channels {}-{})",
                    m.name,
                    c.name,
                    c.start,
                    c.end()
                ));
            }
        }
        if !is_pixel_protocol(m.conn.protocol.as_deref()) {
            warnings.push(format!(
                "'{}' uses protocol '{}', which PixelPlus does not drive; not wired",
                m.name,
                m.conn.protocol.as_deref().unwrap_or("")
            ));
            continue;
        }
        if m.conn.port == 0 {
            warnings.push(format!(
                "'{}' has no controller port in xLights; wire it in PixelPlus",
                m.name
            ));
            continue;
        }
        let n = m.shape.nodes;
        let strings = m.shape.physical_strings.clamp(1, n.max(1));
        let starts_1: Vec<u32> = match &m.shape.string_start_nodes {
            Some(v) if v.len() == strings as usize => v.clone(),
            _ => (0..strings)
                // xLights' ComputeStringStartNode, in single precision like xLights
                // (the rounding decides which pixel starts the next string).
                .map(|s| (s as f32 * (n as f32 / strings as f32) + 1.0) as u32)
                .collect(),
        };
        let mut non_contiguous = false;
        for s in 0..strings {
            let first = starts_1[s as usize].saturating_sub(1).min(n);
            let next = if s + 1 < strings {
                starts_1[s as usize + 1].saturating_sub(1).min(n)
            } else {
                n
            };
            if next <= first {
                if next < first {
                    warnings.push(format!(
                        "'{}': string {} has an invalid start node; skipped",
                        m.name,
                        s + 1
                    ));
                }
                continue;
            }
            let (port, sr) = port_sr(&m.conn, s);
            let nominal = start as u64 + 3 * first as u64;
            let start_ch = match string_starts[mi].as_ref().and_then(|v| v[s as usize]) {
                Some(v) => {
                    non_contiguous |= v as u64 != nominal;
                    v as u64
                }
                None => nominal,
            };
            let entry = controllers.entry(ctrl.clone()).or_default();
            entry.0 = entry.0.max(port);
            entry.1.insert(pi);
            ports.entry((ctrl.clone(), port)).or_default().push(Entry {
                prop: pi,
                sr,
                start_ch,
                offset: first,
                count: next - first,
                nulls: m.conn.nulls,
                end_nulls: m.conn.end_nulls,
                reverse: m.conn.reverse,
            });
        }
        if non_contiguous {
            warnings.push(format!(
                "'{}' uses individual start channels that are not contiguous; its pixels are mapped as one continuous block",
                m.name
            ));
        }
    }
    for ((ctrl, port), mut entries) in ports {
        entries.sort_by_key(|e| (e.sr, e.start_ch, e.prop));
        if entries.iter().any(|e| e.sr > 0) {
            let letters: BTreeSet<String> = entries.iter().map(|e| smart_letter(e.sr)).collect();
            warnings.push(format!(
                "controller '{ctrl}' port {port} uses smart remotes ({}); PixelPlus drives them as one chain on output {port} in remote order",
                letters.into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join(", ")
            ));
        }
        let mut by_channel: Vec<(u64, u64, usize)> = entries
            .iter()
            .map(|e| (e.start_ch, e.start_ch + e.count as u64 * 3, e.prop))
            .collect();
        by_channel.sort_unstable();
        for w in by_channel.windows(2) {
            if w[1].0 < w[0].1 {
                warnings.push(format!(
                    "'{}' and '{}' use overlapping channels on controller '{ctrl}' port {port}",
                    props[w[0].2].name, props[w[1].2].name
                ));
            }
        }
        let mut cursor = 0u32;
        for e in entries {
            cursor = cursor.saturating_add(e.nulls);
            props[e.prop].segments.push(PropSegment {
                node_id: ctrl.clone(),
                output: port,
                start_pixel: cursor,
                pixel_count: e.count,
                prop_offset: e.offset,
                reverse: e.reverse,
                null_pixels: e.nulls,
            });
            cursor = cursor.saturating_add(e.count).saturating_add(e.end_nulls);
        }
    }
    for p in &mut props {
        p.segments.sort_by_key(|s| s.prop_offset);
    }
    let controllers: Vec<ImportController> = controllers
        .into_iter()
        .map(|(name, (ports, props))| {
            let nc = nets.as_ref().and_then(|n| n.controller(&name));
            let ip = nc.and_then(|c| c.ip.clone());
            ImportController {
                suggested_node_id: suggest_node(show, &name, ip.as_deref()),
                protocol: nc.and_then(|c| c.protocol.clone()),
                ip,
                name,
                ports,
                prop_count: props.len() as u32,
            }
        })
        .collect();

    // ---- groups -----------------------------------------------------------------------
    let groups = parse_groups(
        root,
        &models,
        &prop_by_name,
        &mut props,
        show,
        &mut warnings,
    );

    // ---- layout finishing -----------------------------------------------------------
    if let Some((x0, y0, _, _)) = layout::bounds(&props) {
        for l in props.iter_mut().filter_map(|p| p.layout.as_mut()) {
            l.x += CANVAS_MARGIN - x0;
            l.y += CANVAS_MARGIN - y0;
        }
    }
    layout::auto_arrange(&mut props);

    Ok(ImportPreview {
        props,
        controllers,
        groups,
        warnings,
    })
}

fn make_layout(s: &Shape, a: Attrs) -> Option<PropLayout> {
    let world = geometry::world_points(s, a);
    if world.is_empty() || world.len() != s.nodes as usize {
        return None;
    }
    // World is y-up; the canvas is y-down.
    let canvas: Vec<[f32; 2]> = world.iter().map(|p| [p[0], -p[1]]).collect();
    let (origin, size, points) = layout::normalize(&canvas);
    Some(PropLayout {
        x: origin[0],
        y: origin[1],
        w: size[0],
        h: size[1],
        rotation: 0.0,
        points: Some(points),
    })
}

fn parse_groups(
    root: Node,
    models: &[XModel],
    prop_by_name: &HashMap<String, usize>,
    props: &mut [Prop],
    show: &Show,
    warnings: &mut Vec<String>,
) -> Vec<PropGroup> {
    let mut defs: Vec<(String, Vec<String>, Option<String>)> = Vec::new();
    let group_nodes = root
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "modelGroups")
        .flat_map(|g| g.children())
        .chain(
            // Some files keep groups among the models as DisplayAs="ModelGroup".
            root.children()
                .filter(|c| c.is_element() && c.tag_name().name() == "models")
                .flat_map(|m| m.children())
                .filter(|m| m.attribute("DisplayAs") == Some("ModelGroup")),
        )
        .filter(|c| c.is_element());
    for g in group_nodes {
        let a = Attrs(g);
        let Some(name) = a.str("name") else { continue };
        let members = a
            .raw("models")
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let color = a
            .str("TagColour")
            .filter(|c| !c.eq_ignore_ascii_case("#000000") && c.starts_with('#'))
            .map(str::to_string);
        if !defs.iter().any(|d| d.0 == name) {
            defs.push((name.to_string(), members, color));
        }
    }
    let by_name: HashMap<&str, usize> = defs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.0.as_str(), i))
        .collect();
    let model_names: HashSet<&str> = models.iter().map(|m| m.name.as_str()).collect();
    let mut submodel_refs = 0;
    let mut out = Vec::new();
    for (gi, (name, _, color)) in defs.iter().enumerate() {
        let mut members: Vec<usize> = Vec::new();
        let mut member_set: HashSet<usize> = HashSet::new();
        let mut stack = vec![gi];
        let mut visited = HashSet::new();
        while let Some(g) = stack.pop() {
            if !visited.insert(g) {
                continue;
            }
            for m in &defs[g].1 {
                if let Some(&pi) = prop_by_name.get(m) {
                    if member_set.insert(pi) {
                        members.push(pi);
                    }
                } else if let Some(&sub) = by_name.get(m.as_str()) {
                    stack.push(sub);
                } else if m.contains('/') {
                    submodel_refs += 1;
                } else if !model_names.contains(m.as_str()) {
                    warnings.push(format!("group '{name}' refers to unknown model '{m}'"));
                }
            }
        }
        if members.is_empty() {
            continue;
        }
        let id = show
            .prop_groups
            .iter()
            .find(|g| &g.name == name)
            .map(|g| g.id.clone())
            .unwrap_or_else(new_id);
        members.sort_unstable();
        for &pi in &members {
            if !props[pi].group_ids.contains(&id) {
                props[pi].group_ids.push(id.clone());
            }
        }
        out.push(PropGroup {
            id,
            name: name.clone(),
            prop_ids: members.iter().map(|&pi| props[pi].id.clone()).collect(),
            color: color.clone(),
        });
    }
    if submodel_refs > 0 {
        warnings.push(format!(
            "{submodel_refs} submodel reference(s) in groups were skipped (PixelPlus groups whole props)"
        ));
    }
    out
}

/// Merge an import preview into `show` and return the new show (version bumped).
///
/// * Props are matched by `xlightsModel`. For existing props the channel range, pixel
///   count, kind, matrix and wiring come from the import; the user's name (if renamed),
///   colour, notes, current limit and layout box are kept. If the pixel count changed,
///   the preview's per-pixel `points` replace the old ones inside the kept box.
/// * If the import produced no wiring for a prop (controller unmapped or no port), its
///   existing segments are kept when they still fit.
/// * Segment `nodeId` placeholders (xLights controller names) are mapped through
///   `controller_map`, then the controller's `suggestedNodeId`; segments that do not map
///   to an existing node are dropped.
/// * Groups are merged by name (member lists are unioned).
/// * Props that were not in the import are left untouched.
pub fn apply_import(
    show: &Show,
    preview: &ImportPreview,
    controller_map: &BTreeMap<String, String>,
) -> Show {
    let mut out = show.clone();
    out.version = show.version.saturating_add(1);
    let node_ids: HashSet<&str> = show.nodes.iter().map(|n| n.id.as_str()).collect();
    let map_node = |ctrl: &str| -> Option<String> {
        controller_map
            .get(ctrl)
            .cloned()
            .or_else(|| {
                preview
                    .controllers
                    .iter()
                    .find(|c| c.name == ctrl)
                    .and_then(|c| c.suggested_node_id.clone())
            })
            // Already a node id (preview edited by the UI)?
            .or_else(|| node_ids.contains(ctrl).then(|| ctrl.to_string()))
            .filter(|id| node_ids.contains(id.as_str()))
    };

    // Preview prop id -> final prop id.
    let mut id_map: HashMap<String, String> = HashMap::new();
    for p in &preview.props {
        let segments: Vec<PropSegment> = p
            .segments
            .iter()
            .filter_map(|s| {
                map_node(&s.node_id).map(|node_id| PropSegment {
                    node_id,
                    ..s.clone()
                })
            })
            .collect();
        let existing = p
            .xlights_model
            .as_deref()
            .and_then(|m| {
                out.props
                    .iter()
                    .position(|e| e.xlights_model.as_deref() == Some(m))
            })
            .or_else(|| {
                out.props
                    .iter()
                    .position(|e| e.id == p.id && e.xlights_model.is_none())
            });
        match existing {
            Some(i) => {
                let e = &mut out.props[i];
                id_map.insert(p.id.clone(), e.id.clone());
                let renamed = Some(e.name.as_str()) != e.xlights_model.as_deref();
                if !renamed {
                    e.name = p.name.clone();
                }
                let count_changed = e.pixel_count != p.pixel_count;
                e.kind = p.kind;
                e.pixel_count = p.pixel_count;
                e.channel_start = p.channel_start;
                e.channels_per_pixel = p.channels_per_pixel;
                e.xlights_model = p.xlights_model.clone();
                e.matrix = p.matrix.clone();
                if !segments.is_empty() {
                    e.segments = segments;
                } else {
                    let fits = e
                        .segments
                        .iter()
                        .all(|s| s.prop_offset.saturating_add(s.pixel_count) <= e.pixel_count);
                    if !fits {
                        e.segments.clear();
                    }
                }
                if e.color.is_none() {
                    e.color = p.color.clone();
                }
                if e.notes.is_none() {
                    e.notes = p.notes.clone();
                }
                match (&mut e.layout, &p.layout) {
                    (None, l) => e.layout = l.clone(),
                    (Some(el), Some(pl)) if count_changed => el.points = pl.points.clone(),
                    (Some(el), None) if count_changed => el.points = None,
                    _ => {}
                }
            }
            None => {
                let mut np = p.clone();
                np.segments = segments;
                np.group_ids.clear();
                if out.props.iter().any(|e| e.id == np.id) {
                    np.id = new_id();
                }
                id_map.insert(p.id.clone(), np.id.clone());
                out.props.push(np);
            }
        }
    }

    for g in &preview.groups {
        let members: Vec<String> = g
            .prop_ids
            .iter()
            .filter_map(|id| id_map.get(id).cloned())
            .collect();
        let gid = match out.prop_groups.iter_mut().find(|e| e.name == g.name) {
            Some(e) => {
                for m in &members {
                    if !e.prop_ids.contains(m) {
                        e.prop_ids.push(m.clone());
                    }
                }
                if e.color.is_none() {
                    e.color = g.color.clone();
                }
                e.id.clone()
            }
            None => {
                let id = if out.prop_groups.iter().any(|e| e.id == g.id) {
                    new_id()
                } else {
                    g.id.clone()
                };
                out.prop_groups.push(PropGroup {
                    id: id.clone(),
                    name: g.name.clone(),
                    prop_ids: members.clone(),
                    color: g.color.clone(),
                });
                id
            }
        };
        for p in out.props.iter_mut().filter(|p| members.contains(&p.id)) {
            if !p.group_ids.contains(&gid) {
                p.group_ids.push(gid.clone());
            }
        }
    }
    layout::auto_arrange(&mut out.props);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_sr_matches_xlights() {
        let c = Conn {
            port: 1,
            ..Conn::default()
        };
        assert_eq!(port_sr(&c, 0), (1, 0));
        assert_eq!(port_sr(&c, 3), (4, 0));
        let c = Conn {
            port: 1,
            smart_remote: 1,
            sr_max_cascade: 2,
            sr_cascade_on_port: true,
            ..Conn::default()
        };
        assert_eq!(port_sr(&c, 1), (1, 2));
        assert_eq!(port_sr(&c, 2), (2, 1));
        let c = Conn {
            port: 3,
            smart_remote: 1,
            sr_max_cascade: 2,
            ..Conn::default()
        };
        // Ports 3,4 on remote A, then back to port 1 on remote B.
        assert_eq!(port_sr(&c, 1), (4, 1));
        assert_eq!(port_sr(&c, 2), (1, 2));
    }

    #[test]
    fn rejects_non_rgbeffects() {
        let show = Show::default();
        assert_eq!(
            import_preview("<xrgb><settings/></xrgb>", None, &show).unwrap_err(),
            ImportError::NotRgbEffects
        );
        assert!(matches!(
            import_preview("<xrgb><models>", None, &show),
            Err(ImportError::RgbEffectsXml(_))
        ));
        assert!(matches!(
            import_preview("<xrgb><models/></xrgb>", Some("<<"), &show),
            Err(ImportError::NetworksXml(_))
        ));
    }

    #[test]
    fn start_channel_forms_and_cycles() {
        let xml = r#"<xrgb><models>
          <model name="A" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel="1"/>
          <model name="B" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel=">A:1"/>
          <model name="C" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel="@B:4"/>
          <model name="D" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel=">E:1"/>
          <model name="E" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel=">D:1"/>
          <model name="F" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel="!Nope:1"/>
          <model name="G" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel="0"/>
        </models></xrgb>"#;
        let p = import_preview(xml, None, &Show::default()).unwrap();
        let cs: HashMap<&str, u32> = p
            .props
            .iter()
            .map(|p| (p.name.as_str(), p.channel_start))
            .collect();
        assert_eq!(cs["A"], 0);
        assert_eq!(cs["B"], 30);
        assert_eq!(cs["C"], 33);
        assert!(!cs.contains_key("D") && !cs.contains_key("E"));
        assert!(!cs.contains_key("F") && !cs.contains_key("G"));
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("refers back to itself")));
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("need xlights_networks.xml")));
    }

    #[test]
    fn hostile_input_does_not_panic() {
        let xml = r##"<xrgb><models>
          <model name="A" DisplayAs="Matrix" parm1="99999999" parm2="99999999" parm3="0" StartChannel="1"/>
          <model name="B" DisplayAs="Spinner" parm1="1000000" parm2="1000000" parm3="1000000" StartChannel="99999999999999999999"/>
          <model name="C" DisplayAs="Poly Line" parm2="100000" DropPattern="-9223372036854775808,5" NumPoints="2" PointData="nan,inf,0,1,1,1" StartChannel="@G:99999999999999999999"/>
          <model name="D" DisplayAs="Custom" CustomModelCompressed="5,99999,3;1,-1,2;x;2,0,0,1" StartChannel="!:1"/>
          <model name="E" DisplayAs="Window Frame" parm1="1000000" parm2="1000000" parm3="1000000" StartChannel="#:"/>
          <model name="F" DisplayAs="Arches" parm1="2" parm2="10" LayerSizes="999999,999999" StartChannel="&gt;F:1"/>
          <model name="G" DisplayAs="Poly Line" NumPoints="3" PointData="0,0,0,1,0,0,2,0,0" Seg1="1000000" Seg2="1000000" StartChannel="1"/>
          <model name="H" DisplayAs="Cube" parm1="1000000" parm2="1000000" parm3="1000000" StartChannel="1"/>
          <model name="I" DisplayAs="Tree" TreeType="0" TreeDegrees="-5" parm1="3" parm2="0" StartChannel="1" Controller="X">
            <ControllerConnection Port="99999999999" SmartRemote="30" SRMaxCascade="0" nullNodes="-4"/></model>
          <model name="J" DisplayAs="Circle" parm1="2" parm2="5" circleSizes="3,3,3,1" StartChannel="4294967295"/>
          <model name="K" DisplayAs="Star" parm1="1" parm2="7" parm3="1" starRatio="0" StartChannel="-5"/>
          <model DisplayAs="Arches"/>
          <model name="L" DisplayAs="Wreath" StartChannel="#1.2.3.4:x:y" ScaleX="-1" RotateZ="1e30" WorldPosX="1e39"/>
        </models>
        <modelGroups><modelGroup name="G1" models=",,,A,,G1"/><modelGroup models="A"/></modelGroups></xrgb>"##;
        let p = import_preview(xml, Some("<Networks/>"), &Show::default()).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("unreasonably large")));
        for prop in &p.props {
            let l = prop.layout.as_ref().unwrap();
            assert!(l.x.is_finite() && l.y.is_finite() && l.w.is_finite() && l.h.is_finite());
            assert_eq!(l.points.as_ref().unwrap().len(), prop.pixel_count as usize);
        }
        let s = apply_import(&Show::default(), &p, &BTreeMap::new());
        assert_eq!(s.props.len(), p.props.len());
    }

    #[test]
    fn long_start_channel_chains_do_not_recurse() {
        // xLights defaults new models to ">Previous:1", so chains of thousands of
        // models are normal. Resolving them used to recurse once per link.
        // Listed last-first, so resolving the first model walks the whole chain.
        let mut xml = String::from("<xrgb><models>");
        let n = 20_000;
        for i in (1..n).rev() {
            xml.push_str(&format!(
                r#"<model name="M{i}" DisplayAs="Single Line" parm1="1" parm2="2" StartChannel="&gt;M{}:1"/>"#,
                i - 1
            ));
        }
        xml.push_str(
            r#"<model name="M0" DisplayAs="Single Line" parm1="1" parm2="2" StartChannel="1"/>"#,
        );
        xml.push_str("</models></xrgb>");
        let p = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || import_preview(&xml, None, &Show::default()).unwrap())
            .unwrap()
            .join()
            .expect("import must not overflow the stack");
        assert_eq!(p.props.len(), n);
        let last = p
            .props
            .iter()
            .find(|p| p.name == format!("M{}", n - 1))
            .unwrap();
        assert_eq!(last.channel_start, 6 * (n as u32 - 1));
    }

    #[test]
    fn chaining_after_labels_and_uneven_string_splits_match_xlights() {
        let xml = r#"<xrgb><models>
          <model name="Label" DisplayAs="Label" StringType="RGB Nodes" StartChannel="1"/>
          <model name="After" DisplayAs="Single Line" NumStrings="1" NodesPerString="4" StartChannel="&gt;Label:1"/>
          <model name="Poly" DisplayAs="Poly Line" NodesPerString="62" PolyStrings="14" DropPattern="1" NumPoints="2" PointData="0,0,0,10,0,0" StartChannel="100" Controller="C">
            <ControllerConnection Port="1" Protocol="WS2811"/></model>
        </models></xrgb>"#;
        let p = import_preview(xml, None, &Show::default()).unwrap();
        let get = |n: &str| p.props.iter().find(|p| p.name == n).unwrap();
        // A label is one RGB node: the next model starts on channel 4.
        assert_eq!(get("After").channel_start, 3);
        // String 8 of 62 nodes over 14 strings starts at node 31 in xLights'
        // single-precision ComputeStringStartNode (not 32).
        let poly = get("Poly");
        assert_eq!(poly.pixel_count, 62);
        let s8 = poly.segments.iter().find(|s| s.output == 8).unwrap();
        assert_eq!(s8.prop_offset, 30);
    }

    #[test]
    fn xml_depth_scan() {
        let ok = br#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY a "b">]><a><!-- <b><c> --><b x="1>2" y='/>'><![CDATA[<d><e>]]><c/></b></a>"#;
        assert!(!xml_depth_exceeds(ok, 2));
        assert!(xml_depth_exceeds(ok, 1));
        let deep = format!("<a>{}</a>", "<b>".repeat(300));
        assert!(xml_depth_exceeds(deep.as_bytes(), MAX_XML_DEPTH));
        assert!(!xml_depth_exceeds(
            include_bytes!("../testdata/xlights_2025_rgbeffects.xml"),
            8
        ));
    }

    #[test]
    fn overlapping_models_on_a_port_are_reported() {
        let xml = r#"<xrgb><models>
          <model name="A" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel="1" Controller="C">
            <ControllerConnection Port="1"/></model>
          <model name="B" DisplayAs="Single Line" parm1="1" parm2="10" StartChannel="16" Controller="C">
            <ControllerConnection Port="1" Protocol="ws2811"/></model>
        </models></xrgb>"#;
        let p = import_preview(xml, None, &Show::default()).unwrap();
        assert!(p
            .warnings
            .iter()
            .any(|w| w.contains("overlapping channels")));
        assert_eq!(p.props[1].segments[0].start_pixel, 10);
        assert_eq!(p.controllers[0].name, "C");
        assert_eq!(p.controllers[0].ip, None);
    }
}
