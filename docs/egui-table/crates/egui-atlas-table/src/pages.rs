//! Independent viewport reader. The UI caches only requested cell strings; the
//! worker owns a bounded decoded-block LRU. Sparse filtered views therefore do
//! not need one entire row group resident per visible row on the render thread.
use crate::source::{ColumnBlock, Source};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

type Cell = (u32, usize);
type Block = (u32, usize);
const BUDGET: usize = 64 * 1024 * 1024;
const CELL_LIMIT: usize = 8192;
struct Request {
    generation: u64,
    cells: Vec<Cell>,
}
struct Loaded {
    generation: u64,
    cells: Vec<(Cell, String)>,
    error: Option<String>,
}
pub struct PageCache {
    cells: HashMap<Cell, String>,
    order: VecDeque<Cell>,
    pending: HashSet<Cell>,
    viewport: Vec<Cell>,
    tx: SyncSender<Request>,
    rx: Receiver<Loaded>,
    generation: Arc<AtomicU64>,
    decoded_bytes: Arc<AtomicUsize>,
}
impl PageCache {
    pub fn new(store: Arc<Source>, ctx: egui::Context) -> std::io::Result<Self> {
        let (tx, requests) = mpsc::sync_channel::<Request>(1);
        let (results, rx) = mpsc::sync_channel(4);
        let generation = Arc::new(AtomicU64::new(0));
        let token = generation.clone();
        let decoded_bytes = Arc::new(AtomicUsize::new(0));
        let memory = decoded_bytes.clone();
        thread::Builder::new()
            .name("atlas-pages".into())
            .spawn(move || {
                let mut blocks: HashMap<Block, ColumnBlock> = HashMap::new();
                let mut order = VecDeque::new();
                let mut bytes = 0usize;
                while let Ok(mut request) = requests.recv() {
                    while let Ok(newer) = requests.try_recv() {
                        request = newer;
                    }
                    if token.load(Ordering::Relaxed) != request.generation {
                        continue;
                    }
                    let mut grouped: BTreeMap<Block, Vec<u32>> = BTreeMap::new();
                    for (row, column) in request.cells {
                        grouped
                            .entry((row / store.schema.group_rows, column))
                            .or_default()
                            .push(row);
                    }
                    for (key, rows) in grouped {
                        if token.load(Ordering::Relaxed) != request.generation {
                            break;
                        }
                        if let std::collections::hash_map::Entry::Vacant(entry) = blocks.entry(key)
                        {
                            match store.read_block(key.0, key.1) {
                                Ok(block) => {
                                    bytes += block.bytes();
                                    entry.insert(block);
                                }
                                Err(e) => {
                                    if results
                                        .send(Loaded {
                                            generation: request.generation,
                                            cells: Vec::new(),
                                            error: Some(format!("{e:#}")),
                                        })
                                        .is_err()
                                    {
                                        return;
                                    }
                                    ctx.request_repaint();
                                    break;
                                }
                            }
                        }
                        if let Some(i) = order.iter().position(|k| k == &key) {
                            order.remove(i);
                        }
                        order.push_back(key);
                        let block = &blocks[&key];
                        let cells = rows
                            .into_iter()
                            .map(|r| {
                                (
                                    (r, key.1),
                                    block
                                        .value((r % store.schema.group_rows) as usize)
                                        .to_owned(),
                                )
                            })
                            .collect();
                        while bytes > BUDGET {
                            let Some(old) = order.pop_front() else {
                                break;
                            };
                            if let Some(block) = blocks.remove(&old) {
                                bytes -= block.bytes();
                            }
                        }
                        memory.store(bytes, Ordering::Relaxed);
                        if results
                            .send(Loaded {
                                generation: request.generation,
                                cells,
                                error: None,
                            })
                            .is_err()
                        {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
            })?;
        Ok(Self {
            cells: HashMap::new(),
            order: VecDeque::new(),
            pending: HashSet::new(),
            viewport: Vec::new(),
            tx,
            rx,
            generation,
            decoded_bytes,
        })
    }
    pub fn poll(&mut self) -> Option<String> {
        let mut error = None;
        while let Ok(loaded) = self.rx.try_recv() {
            if loaded.generation != self.generation.load(Ordering::Relaxed) {
                continue;
            }
            if let Some(e) = loaded.error {
                error = Some(e);
            }
            for (cell, text) in loaded.cells {
                self.pending.remove(&cell);
                if self.cells.insert(cell, text).is_none() {
                    self.order.push_back(cell);
                }
            }
        }
        error
    }
    pub fn request(&mut self, cells: &[Cell]) {
        if self.viewport != cells {
            self.viewport = cells.to_vec();
            self.pending.clear();
            self.generation.fetch_add(1, Ordering::Relaxed);
            // The bounded UI cache retains visible entries and nearby history.
            while self.cells.len() > CELL_LIMIT {
                let Some(old) = self.order.pop_front() else {
                    break;
                };
                if cells.binary_search(&old).is_err() {
                    self.cells.remove(&old);
                } else {
                    self.order.push_back(old);
                }
                if self.order.len() <= cells.len() {
                    break;
                }
            }
        }
        let missing: Vec<_> = cells
            .iter()
            .filter(|k| !self.cells.contains_key(k) && !self.pending.contains(k))
            .copied()
            .collect();
        if !missing.is_empty() {
            let request = Request {
                generation: self.generation.load(Ordering::Relaxed),
                cells: missing.clone(),
            };
            if self.tx.try_send(request).is_ok() {
                self.pending.extend(missing);
            }
        }
    }
    pub fn value(&self, row: u32, column: usize) -> Option<&str> {
        self.cells.get(&(row, column)).map(String::as_str)
    }
    pub fn is_ready(&self) -> bool {
        self.viewport.iter().all(|c| self.cells.contains_key(c))
    }
    pub fn bytes(&self) -> usize {
        self.decoded_bytes.load(Ordering::Relaxed)
    }
}
impl Drop for PageCache {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}
