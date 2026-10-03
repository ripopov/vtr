//! Shared procedural traces plus viewer completion helpers.
use crate::App;
use std::sync::Arc;
pub use volna_trace::testing::{ProceduralTrace, hierarchy_session};

use crate::session::{LoadRequest, LoadResult};
use volna_trace::Session;
/// Perform all viewer requests, completing opens with `session`.
pub fn complete_open(app: &mut App, session: Arc<dyn Session>) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            app.deliver(match request {
                LoadRequest::Open {
                    trace, generation, ..
                } => LoadResult::Opened {
                    trace,
                    generation,
                    result: Ok(session.clone()),
                },
                request => request.perform(),
            });
        }
    }
}
/// A document showing `hierarchy` as trace A.
pub fn hierarchy_document(hierarchy: impl Into<volna_trace::data::Hierarchy>) -> crate::Document {
    let mut doc = crate::Document::new();
    doc.set_session(hierarchy_session(hierarchy));
    doc
}

/// `item` in trace A, the one trace most tests open.
pub fn a<T>(item: T) -> crate::trace::Traced<T> {
    crate::trace::Traced::new(crate::trace::TraceId::A, item)
}

/// How test workspaces reference their traces: A at `path`, the others
/// by their sources.
pub fn paths(
    path: &str,
) -> impl Fn(&crate::trace::TraceSlot) -> anyhow::Result<Option<String>> + '_ {
    move |slot| {
        Ok(Some(if slot.id.is_a() {
            path.to_owned()
        } else {
            slot.source.clone()
        }))
    }
}

/// Each of `items` in trace A.
pub fn a_all<T>(items: impl IntoIterator<Item = T>) -> Vec<crate::trace::Traced<T>> {
    items.into_iter().map(a).collect()
}
