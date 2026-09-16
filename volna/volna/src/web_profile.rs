//! Opt-in browser measurements using the real document and remote executor.
//! No transaction presentation or alternate decoding path lives here.
use crate::Workspace;
use gpui_kit::Context;
use volna_core::data::transactions::TrackKind;
use volna_core::document::TrackLoadState;
use wasm_bindgen::{JsCast, JsValue};

impl Workspace {
    pub(crate) fn profile_tracks(&mut self, action: &str, cx: &mut Context<Self>) {
        let tracks: Vec<_> = self
            .app
            .doc
            .session()
            .map(|s| {
                s.tracks()
                    .iter()
                    .filter(|t| matches!(t.kind, TrackKind::Stream { .. }))
                    .map(|t| t.id)
                    .collect()
            })
            .unwrap_or_default();
        let mut errors = Vec::new();
        for &track in &tracks {
            match action {
                "load" if self.app.doc.track(track).is_none() => {
                    if let Err(error) = self.app.doc.retain_track(track) {
                        errors.push(format!("{}: {error:#}", track.0));
                    }
                }
                "release" => self.app.doc.release_track(track),
                _ => {}
            }
        }
        let mut ready = 0;
        let mut loading = 0;
        let mut transactions = 0;
        let mut relations = 0;
        for &track in &tracks {
            match self.app.doc.track(track) {
                Some(TrackLoadState::Ready(data)) => {
                    ready += 1;
                    for generator in &data.generators {
                        transactions += generator.transactions().len();
                        relations += generator.relations().len();
                    }
                }
                Some(TrackLoadState::Loading) => loading += 1,
                Some(TrackLoadState::Failed(message)) => {
                    errors.push(format!("{}: {message}", track.0));
                }
                None => {}
            }
        }
        let result = serde_json::json!({
            "action": action, "streams": tracks.len(), "ready": ready,
            "loading": loading, "transactions": transactions,
            "incidentRelations": relations, "errors": errors,
        })
        .to_string();
        if let Ok(callback) =
            js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("volnaProfileResult"))
            && let Some(callback) = callback.dyn_ref::<js_sys::Function>()
        {
            let _ = callback.call1(&JsValue::UNDEFINED, &JsValue::from_str(&result));
        }
        self.after(None, cx);
    }
}
