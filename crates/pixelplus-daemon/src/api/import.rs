//! xLights layout import (ARCHITECTURE §5).
//!
//! `POST /import/xlights` accepts multipart `rgbeffects` (+ optional
//! `networks`). Either field may also be a `.zip` of the xLights show folder;
//! the two XML files are found inside it. Files dropped in the wrong slot are
//! sorted out by their root element.

use super::content::{multipart_error, public_show};
use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::routing::post;
use axum::{Json, Router};
use pixelplus_core::model::Show;
use pixelplus_core::xlights::{apply_import, import_preview, ImportPreview};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;

const MAX_UPLOAD: usize = 200 * 1024 * 1024;
const MAX_XML: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XmlKind {
    RgbEffects,
    Networks,
    Other,
}

fn xml_kind(text: &str) -> XmlKind {
    // Skip the prolog/comments and look at the root element name.
    let mut rest = text.trim_start_matches('\u{feff}');
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("<?") {
            rest = r.split_once("?>").map(|(_, r)| r).unwrap_or("");
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.split_once("-->").map(|(_, r)| r).unwrap_or("");
        } else if let Some(r) = rest.strip_prefix("<!") {
            rest = r.split_once('>').map(|(_, r)| r).unwrap_or("");
        } else {
            break;
        }
    }
    let root: String = rest.trim_start_matches('<').chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    match root.to_ascii_lowercase().as_str() {
        "xrgb" => XmlKind::RgbEffects,
        "networks" => XmlKind::Networks,
        _ => XmlKind::Other,
    }
}

fn decode_text(bytes: Vec<u8>) -> ApiResult<String> {
    String::from_utf8(bytes).or_else(|e| {
        // Some old xLights files are Latin-1.
        Ok(e.into_bytes().iter().map(|&b| b as char).collect())
    })
}

/// Pull the xLights XML files out of a zip (anywhere in the archive).
fn from_zip(bytes: &[u8]) -> ApiResult<(Option<String>, Option<String>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| ApiError::bad_request("That zip file couldn't be opened. Is it complete?"))?;
    let mut rgb = None;
    let mut net = None;
    for i in 0..zip.len() {
        let Ok(mut f) = zip.by_index(i) else { continue };
        let name = f.name().to_ascii_lowercase();
        let base = name.rsplit('/').next().unwrap_or(&name).to_string();
        if name.contains("__macosx") || !(base == "xlights_rgbeffects.xml" || base == "xlights_networks.xml") {
            continue;
        }
        if f.size() > MAX_XML {
            return Err(ApiError::bad_request(format!("{base} in the zip is unexpectedly large.")));
        }
        let mut buf = Vec::new();
        f.by_ref().take(MAX_XML).read_to_end(&mut buf).map_err(|_| ApiError::bad_request("That zip file is damaged."))?;
        let text = decode_text(buf)?;
        if base == "xlights_rgbeffects.xml" && rgb.is_none() {
            rgb = Some(text);
        } else if base == "xlights_networks.xml" && net.is_none() {
            net = Some(text);
        }
    }
    Ok((rgb, net))
}

async fn preview(State(state): State<AppState>, mut mp: Multipart) -> ApiResult<Json<ImportPreview>> {
    let mut rgb: Option<String> = None;
    let mut net: Option<String> = None;
    let mut others: Vec<String> = Vec::new();
    while let Some(field) = mp.next_field().await.map_err(multipart_error)? {
        let slot = field.name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(multipart_error)?;
        if bytes.is_empty() {
            continue;
        }
        if bytes.starts_with(b"PK\x03\x04") {
            let (r, n) = from_zip(&bytes)?;
            if r.is_none() && n.is_none() {
                return Err(ApiError::bad_request(
                    "That zip doesn't contain xlights_rgbeffects.xml. Zip your whole xLights show folder and try again.",
                ));
            }
            rgb = rgb.or(r);
            net = net.or(n);
            continue;
        }
        let text = decode_text(bytes.to_vec())?;
        match (xml_kind(&text), slot.as_str()) {
            (XmlKind::RgbEffects, _) => rgb = Some(text),
            (XmlKind::Networks, _) => net = Some(text),
            (XmlKind::Other, "rgbeffects") => others.push(text),
            _ => {}
        }
    }
    let rgb = match (rgb, others.pop()) {
        (Some(r), _) => r,
        // Unrecognised root: let the importer explain what's wrong.
        (None, Some(o)) => o,
        (None, None) => {
            return Err(ApiError::bad_request(if net.is_some() {
                "That's xlights_networks.xml. Also choose xlights_rgbeffects.xml from your xLights show folder."
            } else {
                "Choose xlights_rgbeffects.xml from your xLights show folder (or a zip of the folder)."
            }))
        }
    };
    let show = state.store.get();
    let preview = tokio::task::spawn_blocking(move || import_preview(&rgb, net.as_deref(), &show))
        .await
        .map_err(ApiError::internal)?
        .map_err(|e| ApiError::bad_request(format!("{e}.")))?;
    Ok(Json(preview))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyBody {
    preview: ImportPreview,
    #[serde(default)]
    controller_map: BTreeMap<String, String>,
}

async fn apply(State(state): State<AppState>, Json(body): Json<ApplyBody>) -> ApiResult<Json<Show>> {
    if body.preview.props.is_empty() && body.preview.groups.is_empty() {
        return Err(ApiError::bad_request("There's nothing to import."));
    }
    {
        let show = state.store.get();
        for (ctrl, node) in &body.controller_map {
            if !node.is_empty() && show.node(node).is_none() {
                return Err(ApiError::bad_request(format!(
                    "The controller chosen for \"{ctrl}\" no longer exists. Pick it again."
                )));
            }
        }
    }
    crate::services::snapshots::auto(&state, "Before xLights import").await;
    let map: BTreeMap<String, String> = body.controller_map.into_iter().filter(|(_, v)| !v.is_empty()).collect();
    let preview = body.preview;
    let (_, show) = state
        .store
        .update(move |s| {
            let mut next = apply_import(s, &preview, &map);
            pixelplus_core::layout::auto_arrange(&mut next.props);
            next.version = s.version;
            *s = next;
            Ok(())
        })
        .await?;
    Ok(Json(public_show(&show)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/import/xlights", post(preview).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/import/xlights/apply", post(apply).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_root_elements() {
        assert_eq!(xml_kind("<?xml version=\"1.0\"?>\n<!-- hi -->\n<xrgb><models/></xrgb>"), XmlKind::RgbEffects);
        assert_eq!(xml_kind("\u{feff}<?xml version=\"1.0\"?><Networks computer=\"x\"/>"), XmlKind::Networks);
        assert_eq!(xml_kind("<html></html>"), XmlKind::Other);
    }

    #[test]
    fn zip_extraction() {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("Show/xlights_rgbeffects.xml", opts).unwrap();
            w.write_all(b"<xrgb><models/></xrgb>").unwrap();
            w.start_file("Show/xlights_networks.xml", opts).unwrap();
            w.write_all(b"<Networks/>").unwrap();
            w.start_file("Show/other.txt", opts).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let (r, n) = from_zip(buf.get_ref()).unwrap();
        assert_eq!(r.as_deref(), Some("<xrgb><models/></xrgb>"));
        assert_eq!(n.as_deref(), Some("<Networks/>"));
    }
}
