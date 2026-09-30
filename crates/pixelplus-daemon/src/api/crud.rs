//! Generic create/read/update/delete endpoints for show collections.
//!
//! Updates are **JSON merge patches** (RFC 7386): the UI sends only the fields
//! it changed, which keeps "edit in place" snappy and conflict-free.

use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use pixelplus_core::model::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// A show collection exposed over CRUD.
pub trait Entity: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// Singular human label, used in error messages ("Prop").
    const LABEL: &'static str;
    fn id(&self) -> &str;
    fn set_id(&mut self, id: String);
    fn list(show: &Show) -> &Vec<Self>;
    fn list_mut(show: &mut Show) -> &mut Vec<Self>;
    /// Validate a created/updated entity against the rest of the show.
    fn validate(&self, _show: &Show) -> ApiResult<()> {
        Ok(())
    }
    /// Remove references to a deleted entity from the rest of the show.
    fn on_delete(_id: &str, _show: &mut Show) {}
}

macro_rules! entity {
    ($t:ty, $label:literal, $field:ident) => {
        impl Entity for $t {
            const LABEL: &'static str = $label;
            fn id(&self) -> &str {
                &self.id
            }
            fn set_id(&mut self, id: String) {
                self.id = id;
            }
            fn list(show: &Show) -> &Vec<Self> {
                &show.$field
            }
            fn list_mut(show: &mut Show) -> &mut Vec<Self> {
                &mut show.$field
            }
            fn validate(&self, show: &Show) -> ApiResult<()> {
                validate_name(&self.name)?;
                entity!(@validate self, show, $field)
            }
            fn on_delete(id: &str, show: &mut Show) {
                entity!(@delete id, show, $field)
            }
        }
    };
    (@validate $s:ident, $show:ident, props) => { validate_prop($s, $show) };
    (@validate $s:ident, $show:ident, receivers) => { validate_receiver($s, $show) };
    (@validate $s:ident, $show:ident, $other:ident) => {{ let _ = $show; Ok(()) }};
    (@delete $id:ident, $show:ident, props) => {{
        for g in &mut $show.prop_groups { g.prop_ids.retain(|p| p != $id); }
        for e in &mut $show.effects { e.target.prop_ids.retain(|p| p != $id); }
        if $show.settings.games.matrix_prop_id.as_deref() == Some($id) {
            $show.settings.games.matrix_prop_id = None;
        }
    }};
    (@delete $id:ident, $show:ident, prop_groups) => {{
        for p in &mut $show.props { p.group_ids.retain(|g| g != $id); }
        for e in &mut $show.effects { e.target.group_ids.retain(|g| g != $id); }
    }};
    (@delete $id:ident, $show:ident, receivers) => {{ let _ = ($id, $show); }};
    (@delete $id:ident, $show:ident, effects) => {{
        for p in &mut $show.playlists {
            for list in [&mut p.items, &mut p.intro, &mut p.outro] {
                list.retain(|i| !matches!(i, PlaylistItem::Effect { effect_id, .. } if effect_id == $id));
            }
        }
        if $show.schedule.idle_effect_id.as_deref() == Some($id) { $show.schedule.idle_effect_id = None; }
        if $show.schedule.off_effect_id.as_deref() == Some($id) { $show.schedule.off_effect_id = None; }
    }};
    (@delete $id:ident, $show:ident, dj_clips) => {{
        for p in &mut $show.playlists {
            for list in [&mut p.items, &mut p.intro, &mut p.outro] {
                list.retain(|i| !matches!(i, PlaylistItem::Dj { dj_clip_id, .. } if dj_clip_id == $id));
            }
        }
    }};
    (@delete $id:ident, $show:ident, playlists) => {{
        $show.schedule.entries.retain(|e| e.playlist_id != $id);
        if $show.settings.requests.playlist_id.as_deref() == Some($id) {
            $show.settings.requests.playlist_id = None;
        }
    }};
    (@delete $id:ident, $show:ident, $other:ident) => {{ let _ = ($id, $show); }};
}

entity!(Receiver, "Receiver", receivers);
entity!(Prop, "Prop", props);
entity!(PropGroup, "Group", prop_groups);
entity!(EffectPreset, "Look", effects);
entity!(Playlist, "Playlist", playlists);
entity!(DjClip, "DJ clip", dj_clips);
entity!(DjVoice, "Voice", dj_voices);

fn validate_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request("Please give it a name."));
    }
    if name.chars().count() > 120 {
        return Err(ApiError::bad_request(
            "That name is too long (120 characters max).",
        ));
    }
    Ok(())
}

fn validate_receiver(r: &Receiver, show: &Show) -> ApiResult<()> {
    let node = show.node(&r.node_id).ok_or_else(|| {
        ApiError::bad_request("Pick the controller this receiver is plugged into.")
    })?;
    let jacks = node.board.jack_count() as u32;
    if r.jack == 0 || (jacks > 0 && r.jack > jacks) {
        return Err(ApiError::bad_request(format!(
            "{} has {} jack{}; pick one between 1 and {jacks}.",
            node.name,
            jacks,
            if jacks == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

fn validate_prop(p: &Prop, show: &Show) -> ApiResult<()> {
    if p.pixel_count == 0 {
        return Err(ApiError::bad_request("A prop needs at least one pixel."));
    }
    for seg in &p.segments {
        let node = show.node(&seg.node_id).ok_or_else(|| {
            ApiError::bad_request("A wiring segment points at a controller that no longer exists.")
        })?;
        if seg.output == 0 || seg.output as usize > node.outputs.len() {
            return Err(ApiError::bad_request(format!(
                "{} doesn't have output {}.",
                node.name, seg.output
            )));
        }
        if seg.prop_offset + seg.pixel_count > p.pixel_count {
            return Err(ApiError::bad_request(format!(
                "The wiring covers more pixels than \"{}\" has ({}).",
                p.name, p.pixel_count
            )));
        }
    }
    Ok(())
}

/// RFC 7386 JSON merge patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

async fn list<E: Entity>(State(state): State<AppState>) -> Json<Vec<E>> {
    Json(E::list(&state.store.get()).clone())
}

async fn get_one<E: Entity>(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<E>> {
    E::list(&state.store.get())
        .iter()
        .find(|e| e.id() == id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found(E::LABEL))
}

async fn create<E: Entity>(
    State(state): State<AppState>,
    Json(mut body): Json<Value>,
) -> ApiResult<Json<E>> {
    if let Value::Object(map) = &mut body {
        let has_id = map
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if !has_id {
            map.insert("id".into(), Value::String(new_id()));
        }
    }
    let entity: E = serde_json::from_value(body).map_err(|e| {
        ApiError::bad_request(format!("That {} isn't valid: {e}", E::LABEL.to_lowercase()))
    })?;
    let (created, _) = state
        .store
        .update(|show| {
            if E::list(show).iter().any(|e| e.id() == entity.id()) {
                return Err(ApiError::conflict(format!(
                    "That {} already exists.",
                    E::LABEL.to_lowercase()
                )));
            }
            entity.validate(show)?;
            E::list_mut(show).push(entity.clone());
            Ok(entity)
        })
        .await?;
    Ok(Json(created))
}

async fn update<E: Entity>(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(patch): Json<Value>,
) -> ApiResult<Json<E>> {
    let (updated, _) = state
        .store
        .update(|show| {
            let idx = E::list(show)
                .iter()
                .position(|e| e.id() == id)
                .ok_or_else(|| ApiError::not_found(E::LABEL))?;
            let mut value =
                serde_json::to_value(&E::list(show)[idx]).map_err(ApiError::internal)?;
            merge_patch(&mut value, &patch);
            let mut entity: E = serde_json::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("That change isn't valid: {e}")))?;
            entity.set_id(id.clone());
            entity.validate(show)?;
            E::list_mut(show)[idx] = entity.clone();
            Ok(entity)
        })
        .await?;
    Ok(Json(updated))
}

async fn delete<E: Entity>(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .store
        .update(|show| {
            let before = E::list(show).len();
            E::list_mut(show).retain(|e| e.id() != id);
            if E::list(show).len() == before {
                return Err(ApiError::not_found(E::LABEL));
            }
            E::on_delete(&id, show);
            Ok(())
        })
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Mount CRUD routes for `E` at `/{path}` and `/{path}/{id}`.
pub fn routes<E: Entity>(path: &str) -> Router<AppState> {
    Router::new()
        .route(&format!("/{path}"), get(list::<E>).post(create::<E>))
        .route(
            &format!("/{path}/{{id}}"),
            get(get_one::<E>)
                .put(update::<E>)
                .patch(update::<E>)
                .delete(delete::<E>),
        )
}

/// All collection routes.
pub fn all_routes() -> Router<AppState> {
    Router::new()
        .merge(routes::<Receiver>("receivers"))
        .merge(routes::<Prop>("props"))
        .merge(routes::<PropGroup>("prop-groups"))
        .merge(routes::<EffectPreset>("effects"))
        .merge(routes::<Playlist>("playlists"))
        .merge(routes::<DjClip>("dj-clips"))
        .merge(routes::<DjVoice>("dj-voices"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_patch_rfc7386() {
        let mut t = json!({"a": 1, "b": {"c": 2, "d": 3}});
        merge_patch(&mut t, &json!({"a": 5, "b": {"c": null, "e": 4}}));
        assert_eq!(t, json!({"a": 5, "b": {"d": 3, "e": 4}}));
    }
}
