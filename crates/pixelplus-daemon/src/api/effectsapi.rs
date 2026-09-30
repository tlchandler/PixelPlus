//! Effect catalogue and live preview of looks.

use super::{ApiError, ApiResult};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use pixelplus_core::effects::{
    builtin_presets, effect_catalog, param_schema, EffectInfo, ALL_EFFECT_KINDS,
};
use pixelplus_core::model::EffectPreset;
use serde::Deserialize;
use serde_json::{json, Map, Value};

async fn catalog() -> Json<Vec<EffectInfo>> {
    Json(effect_catalog())
}

async fn schema() -> Json<Value> {
    let mut m = Map::new();
    for kind in ALL_EFFECT_KINDS {
        let key = serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        m.insert(
            key,
            serde_json::to_value(param_schema(kind)).unwrap_or(Value::Null),
        );
    }
    Json(Value::Object(m))
}

async fn builtin() -> Json<Vec<EffectPreset>> {
    Json(builtin_presets())
}

#[derive(Deserialize)]
struct PreviewBody {
    #[serde(default)]
    preset: Option<EffectPreset>,
    #[serde(default)]
    effect: Option<EffectPreset>,
}

/// Live-apply a (possibly unsaved) look; `{preset: null}` stops it.
async fn preview_apply(
    State(state): State<AppState>,
    Json(b): Json<PreviewBody>,
) -> ApiResult<Json<Value>> {
    let preset = b.preset.or(b.effect);
    if let Some(p) = &preset {
        if p.name.len() > 200 {
            return Err(ApiError::bad_request("That look's name is too long."));
        }
    }
    super::playerapi::apply_effect(&state, preset).await?;
    Ok(Json(json!({ "ok": true })))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/effects/catalog", get(catalog))
        .route("/effects/schema", get(schema))
        .route("/effects/builtin", get(builtin))
        .route("/effects/preview-apply", post(preview_apply))
}
