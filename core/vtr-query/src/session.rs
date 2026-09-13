//! Typed session requests and deliveries, independent of execution and framing.
use crate::metadata::{DeclarationPage, Path, ResolvePage, SearchPage, TextPart};
use crate::wave::WindowPage;
use crate::{Grid, Interval, Reservation};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SnapshotId(pub [u8; 16]);

/// A cursor belongs to one operation on one opened snapshot. Fields may be
/// transported as integers/bytes but must always be validated by the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Continuation {
    pub snapshot: SnapshotId,
    pub operation: u64,
    pub step: u64,
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub snapshot: SnapshotId,
    pub timescale: i8,
    pub time_range: Option<Interval>,
    pub signals: u32,
    pub declarations: u64,
}

#[derive(Debug)]
pub enum Query {
    Window { signal: u32, interval: Interval },
    Summary { signal: u32, grid: Grid },
    Children { parent: Option<u32> },
    Search { scope: Option<u32>, needle: String },
    Resolve { paths: Vec<Path> },
    Text { id: u32, offset: u64, length: usize },
}

#[derive(Debug)]
pub enum Reply {
    Window(Arc<WindowPage>),
    Summary(Arc<crate::summary::SummaryPage>),
    Children(Arc<DeclarationPage>),
    Search(Arc<SearchPage>),
    Resolve(Arc<ResolvePage>),
    Text(Arc<TextPart>),
}
impl Reply {
    pub fn complete(&self) -> bool {
        match self {
            Self::Window(page) => page.complete,
            Self::Summary(page) => page.complete,
            Self::Children(page) => page.complete,
            Self::Search(page) => page.complete,
            Self::Resolve(page) => page.complete,
            Self::Text(_) => true,
        }
    }
}
#[derive(Debug)]
pub struct Delivery {
    pub request: Continuation,
    pub next: Option<Continuation>,
    pub reply: Reply,
    pub(crate) _charge: Reservation,
}
