//! Opt-in browser measurements using the real document and remote executor.
//! No transaction presentation or alternate decoding path lives here.
use crate::Workspace;
use gpui_kit::Context;
use volna_core::data::transactions::TrackKind;
use volna_core::document::TrackLoadState;
use volna_core::trace::Traced;
use wasm_bindgen::{JsCast, JsValue};

impl Workspace {
    pub(crate) fn profile_tracks(&mut self, action: &str, cx: &mut Context<Self>) {
        let mut tracks = Vec::new();
        for trace in self.app.doc.traces().ids() {
            if let Some(session) = self.app.doc.session(trace) {
                tracks.extend(
                    session
                        .tracks()
                        .iter()
                        .filter(|t| matches!(t.kind, TrackKind::Stream { .. }))
                        .map(|t| Traced::new(trace, t.id)),
                );
            }
        }
        let mut errors = Vec::new();
        for &track in &tracks {
            match action {
                "load" if self.app.doc.track(track).is_none() => {
                    if let Err(error) = self.app.doc.retain_track(track) {
                        errors.push(format!("{}:{}: {error:#}", track.trace, track.item.0));
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
                    errors.push(format!("{}:{}: {message}", track.trace, track.item.0));
                }
                None => {}
            }
        }
        // Thin diagnostic projection of the same immutable session queries.
        let hierarchies: Vec<_> = self.app.doc.traces().loaded().map(|(trace, session)| {
            let h = session.hierarchy();
            let roots: Vec<_> = h.roots().iter().map(|id| {
                let scope = h.scope(id);
                serde_json::json!({"id": id, "name": scope.name, "component": scope.component,
                    "children": scope.children.len(), "vars": scope.vars.len(),
                    "size": self.app.doc.traces().get(trace).and_then(|s| s.sizes()).and_then(|s| s.get(id))})
            }).collect();
            serde_json::json!({"scopes": h.scope_count(), "vars": h.var_count(), "roots": roots,
                "lastScope": h.get_scope(h.scope_count().saturating_sub(1)).map(|s| (s.name, s.kind, s.component, s.parent)),
                "lastVar": h.get_var(h.var_count().saturating_sub(1)).map(|v| (v.name, v.scope, v.signal.0, v.enum_table))})
        }).collect();
        let result = serde_json::json!({
            "action": action, "streams": tracks.len(), "ready": ready,
            "loading": loading, "transactions": transactions,
            "incidentRelations": relations, "errors": errors, "hierarchies": hierarchies,
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
