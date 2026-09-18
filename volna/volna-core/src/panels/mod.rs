//! Panel identity, ownership, and docking operations. Widget IDs and pixel
//! geometry never enter this model.

mod layout;
pub use layout::{Axis, Layout, MAX_LAYOUT_DEPTH, MAX_PANELS};

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::wave::model::{LinkDim, WaveModel};

#[derive(Clone, Debug, PartialEq)]
pub enum PanelsCommand {
    Split { panel: PanelId, axis: Axis },
    NewTab { group_of: PanelId },
    Close(PanelId),
    CloseOthers(PanelId),
    Focus(PanelId),
    FocusNext,
    FocusPrev,
    FocusIndex(usize),
    ToggleLink { panel: PanelId, dim: LinkDim },
    Rename(PanelId, Option<String>),
    SetLayout { layout: Layout, from_revision: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PanelId(pub u64);

pub enum PanelKind {
    Waves(Box<WaveModel>),
    /// The settings editor: a chrome tab the workspace codec never saves.
    Settings,
    /// Unrecognized panel payloads are preserved by the workspace codec.
    Unsupported(Box<serde_json::value::RawValue>),
}

impl PanelKind {
    pub fn waves(&self) -> Option<&WaveModel> {
        match self {
            Self::Waves(w) => Some(w),
            _ => None,
        }
    }

    pub fn waves_mut(&mut self) -> Option<&mut WaveModel> {
        match self {
            Self::Waves(w) => Some(w),
            _ => None,
        }
    }

    pub fn is_settings(&self) -> bool {
        matches!(self, Self::Settings)
    }
}

pub struct Panel {
    pub id: PanelId,
    pub title: Option<String>,
    pub kind: PanelKind,
}

impl Panel {
    fn new(id: PanelId, kind: PanelKind) -> Self {
        Self {
            id,
            title: None,
            kind,
        }
    }

    pub fn title(&self) -> String {
        self.title.clone().unwrap_or_else(|| match self.kind {
            PanelKind::Waves(_) => format!("Waves {}", self.id.0),
            PanelKind::Settings => "Settings".into(),
            PanelKind::Unsupported(_) => format!("Unsupported panel {}", self.id.0),
        })
    }
}

pub struct Panels {
    layout: Layout,
    panels: BTreeMap<PanelId, Panel>,
    focused: PanelId,
    next_id: u64,
    revision: u64,
}

impl Default for Panels {
    fn default() -> Self {
        Self::new()
    }
}

impl Panels {
    pub fn new() -> Self {
        let id = PanelId(1);
        Self {
            layout: Layout::single(id),
            panels: BTreeMap::from([(id, Panel::new(id, PanelKind::Waves(Box::default())))]),
            focused: id,
            next_id: 2,
            revision: 0,
        }
    }

    /// Assemble a validated workspace without changing an existing panel collection.
    pub(crate) fn restore(
        layout: Layout,
        panels: Vec<Panel>,
        focused: PanelId,
        previous: &Self,
    ) -> Result<Self> {
        let count = panels.len();
        let panels: BTreeMap<_, _> = panels.into_iter().map(|p| (p.id, p)).collect();
        ensure!(panels.len() == count, "duplicate panel ID");
        let next_id = panels
            .keys()
            .map(|p| p.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("panel IDs exhausted"))?;
        let mut result = Self {
            layout,
            panels,
            focused,
            next_id: next_id.max(previous.next_id),
            revision: previous
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("layout revision exhausted"))?,
        };
        result.validate()?;
        result.layout = result.layout.normalized().expect("validated layout");
        result.validate()?;
        if let Some(id) = previous.settings_id() {
            result.readd_settings(id, previous.focused == id)?;
        }
        Ok(result)
    }

    /// The open settings tab, if any.
    pub fn settings_id(&self) -> Option<PanelId> {
        self.iter().find(|p| p.kind.is_settings()).map(|p| p.id)
    }

    /// Open the settings tab in the focused group, or focus it. Returns
    /// whether the layout changed.
    pub fn open_settings(&mut self) -> Result<bool> {
        if let Some(id) = self.settings_id() {
            return self.focus(id);
        }
        ensure!(self.len() < MAX_PANELS, "panel limit reached");
        let id = self.unused_id()?;
        self.advance()?;
        self.next_id = id.0 + 1;
        self.insert_settings(id, self.focused, true);
        Ok(true)
    }

    /// Put a settings panel with this id beside `beside` and give it focus if asked.
    fn insert_settings(&mut self, id: PanelId, beside: PanelId, focus: bool) {
        if let Some(Layout::Tabs { tabs, active }) = self.layout.group_mut(beside) {
            tabs.push(id);
            if focus {
                *active = id;
            }
        }
        self.panels.insert(id, Panel::new(id, PanelKind::Settings));
        if focus {
            self.focused = id;
        }
    }

    /// Restore the settings tab after a trace change or a workspace restore
    /// replaced the panels; its id is kept so the frontend keeps its view.
    fn readd_settings(&mut self, id: PanelId, focus: bool) -> Result<()> {
        ensure!(!self.panels.contains_key(&id), "settings id reused");
        ensure!(self.len() < MAX_PANELS, "panel limit reached");
        self.next_id = self.next_id.max(id.0 + 1);
        let beside = self.focused;
        self.insert_settings(id, beside, focus);
        self.validate()
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn focused_id(&self) -> PanelId {
        self.focused
    }
    pub fn focused(&self) -> &Panel {
        &self.panels[&self.focused]
    }
    pub fn focused_mut(&mut self) -> &mut Panel {
        self.panels.get_mut(&self.focused).unwrap()
    }

    pub fn waves(&self, id: PanelId) -> Option<&WaveModel> {
        self.get(id)?.kind.waves()
    }

    pub fn focused_waves(&self) -> Option<&WaveModel> {
        self.waves(self.focused)
    }

    pub fn focused_waves_mut(&mut self) -> Option<&mut WaveModel> {
        self.waves_mut(self.focused)
    }

    pub fn waves_mut(&mut self, id: PanelId) -> Option<&mut WaveModel> {
        self.get_mut(id)?.kind.waves_mut()
    }
    pub fn get(&self, id: PanelId) -> Option<&Panel> {
        self.panels.get(&id)
    }
    pub fn get_mut(&mut self, id: PanelId) -> Option<&mut Panel> {
        self.panels.get_mut(&id)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Panel> {
        self.panels.values()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Panel> {
        self.panels.values_mut()
    }
    pub fn len(&self) -> usize {
        self.panels.len()
    }
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    fn ids(&self) -> BTreeSet<PanelId> {
        self.panels.keys().copied().collect()
    }

    /// The next panel ID, reserved by the caller with `next_id = id.0 + 1`.
    fn unused_id(&self) -> Result<PanelId> {
        self.next_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("panel IDs exhausted"))?;
        Ok(PanelId(self.next_id))
    }

    pub fn validate(&self) -> Result<()> {
        self.layout.validate(&self.ids())?;
        ensure!(
            self.layout.visible().contains(&self.focused),
            "focused panel is not visible"
        );
        ensure!(
            self.panels.values().any(|p| p.kind.waves().is_some()),
            "workspace requires a waveform panel"
        );
        Ok(())
    }

    fn advance(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("layout revision exhausted"))?;
        Ok(())
    }

    /// User dock edits are all-or-nothing and must describe the same panels.
    pub fn set_layout(&mut self, layout: Layout, from_revision: u64) -> Result<bool> {
        ensure!(from_revision == self.revision, "stale layout proposal");
        layout.validate(&self.ids())?;
        let layout = layout.normalized().expect("validated nonempty tree");
        layout.validate(&self.ids())?;
        if layout == self.layout {
            return Ok(false);
        }
        self.advance()?;
        let visible = layout.visible();
        if !visible.contains(&self.focused) {
            self.focused = layout.active_for(self.focused).unwrap_or(visible[0]);
        }
        self.layout = layout;
        Ok(true)
    }

    pub fn focus(&mut self, id: PanelId) -> Result<bool> {
        ensure!(self.panels.contains_key(&id), "unknown panel");
        if self.focused == id {
            return Ok(false);
        }
        self.advance()?;
        self.layout.activate(id);
        self.focused = id;
        Ok(true)
    }

    /// Cycles across groups as well as hidden tabs, in tree order.
    pub fn focus_next(&mut self, backwards: bool) -> Result<bool> {
        let ids = self.layout.panels();
        let ix = ids.iter().position(|p| *p == self.focused).unwrap();
        let next = if backwards {
            (ix + ids.len() - 1) % ids.len()
        } else {
            (ix + 1) % ids.len()
        };
        self.focus(ids[next])
    }

    pub fn focus_index(&mut self, ix: usize) -> Result<bool> {
        match self.layout.panels().get(ix) {
            Some(&id) => self.focus(id),
            None => Ok(false),
        }
    }

    pub fn rename(&mut self, id: PanelId, title: Option<String>) -> Result<bool> {
        let panel = self
            .panels
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown panel"))?;
        let title = title.filter(|s| !s.trim().is_empty());
        if panel.title == title {
            return Ok(false);
        }
        let opaque = if let PanelKind::Unsupported(raw) = &panel.kind {
            let mut fields: BTreeMap<String, Box<serde_json::value::RawValue>> =
                serde_json::from_str(raw.get())?;
            fields.insert("title".into(), serde_json::value::to_raw_value(&title)?);
            Some(serde_json::value::to_raw_value(&fields)?)
        } else {
            None
        };
        self.advance()?;
        let panel = self.panels.get_mut(&id).unwrap();
        panel.title = title;
        if let Some(raw) = opaque {
            panel.kind = PanelKind::Unsupported(raw);
        }
        Ok(true)
    }

    pub fn toggle_link(&mut self, id: PanelId, doc: &crate::Document, dim: LinkDim) -> Result<()> {
        ensure!(self.waves(id).is_some(), "not a waveform panel");
        self.advance()?;
        self.waves_mut(id).unwrap().toggle_link(doc, dim);
        Ok(())
    }

    /// Splits clone content; new tabs start empty. Both inherit navigation.
    pub fn create(&mut self, source: PanelId, split: Option<Axis>) -> Result<PanelId> {
        ensure!(self.len() < MAX_PANELS, "panel limit reached");
        let id = self.unused_id()?;
        let src = self
            .panels
            .get(&source)
            .ok_or_else(|| anyhow::anyhow!("unknown panel"))?;
        let waves = match src.kind.waves() {
            Some(w) => w.clone_view(split.is_some()),
            None if split.is_none() => WaveModel::new(),
            None => bail!("cannot clone an unsupported panel"),
        };
        let mut layout = self.layout.clone();
        if let Some(axis) = split {
            layout.split(source, id, axis);
        } else if let Some(Layout::Tabs { tabs, active }) = layout.group_mut(source) {
            tabs.push(id);
            *active = id;
        }
        let layout = layout.normalized().unwrap();
        let mut ids = self.ids();
        ids.insert(id);
        layout.validate(&ids)?;
        self.advance()?;
        self.next_id = id.0 + 1;
        self.layout = layout;
        self.focused = id;
        self.panels
            .insert(id, Panel::new(id, PanelKind::Waves(Box::new(waves))));
        Ok(id)
    }

    pub fn close(&mut self, id: PanelId) -> Result<()> {
        let panel = self
            .panels
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown panel"))?;
        // Keep a usable waveform view, including alongside unknown kinds.
        if panel.kind.waves().is_some()
            && self.iter().filter(|p| p.kind.waves().is_some()).count() == 1
        {
            self.advance()?;
            let w = self.panels.get_mut(&id).unwrap().kind.waves_mut().unwrap();
            *w = w.clone_view(false);
            return Ok(());
        }
        let ids = self.layout.panels();
        let ix = ids.iter().position(|p| *p == id).unwrap();
        let mut layout = self.layout.clone();
        let neighbor = layout.remove(id);
        let layout = layout.normalized().unwrap();
        let mut survivors = self.ids();
        survivors.remove(&id);
        layout.validate(&survivors)?;
        self.advance()?;
        self.layout = layout;
        self.panels.remove(&id);
        if self.focused == id {
            self.focused = neighbor.unwrap_or_else(|| ids[(ix + 1) % ids.len()]);
            self.layout.activate(self.focused);
        }
        Ok(())
    }

    pub fn close_others(&mut self, id: PanelId) -> Result<()> {
        let panel = self
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown panel"))?;
        let mut keep = BTreeSet::from([id]);
        if panel.kind.waves().is_none() {
            keep.insert(self.iter().find(|p| p.kind.waves().is_some()).unwrap().id);
        }
        if self.len() == 1 {
            return Ok(());
        }
        self.advance()?;
        self.panels.retain(|id, _| keep.contains(id));
        self.layout = Layout::Tabs {
            tabs: keep.into_iter().collect(),
            active: id,
        };
        self.focused = id;
        Ok(())
    }

    /// The layout, focus and panels a workspace file records: everything but
    /// the settings tab, which is chrome.
    pub fn saved_view(&self) -> (Layout, PanelId, Vec<&Panel>) {
        let mut layout = self.layout.clone();
        for id in self.iter().filter(|p| p.kind.is_settings()).map(|p| p.id) {
            layout.remove(id);
        }
        let layout = layout.normalized().expect("a waveform panel remains");
        let focused = if self.panels[&self.focused].kind.is_settings() {
            layout
                .active_for(self.focused)
                .unwrap_or_else(|| layout.visible()[0])
        } else {
            self.focused
        };
        let panels = self.iter().filter(|p| !p.kind.is_settings()).collect();
        (layout, focused, panels)
    }

    /// A trace change discards content while keeping the ID allocator alive,
    /// so delayed pointer events can never address a replacement panel. The
    /// settings tab survives the change.
    pub fn reset(&mut self, limits: Option<(u64, u64)>) -> Result<()> {
        let id = self.unused_id()?;
        self.advance()?;
        let settings = self.settings_id().map(|id| (id, self.focused == id));
        self.next_id = id.0 + 1;
        let mut waves = WaveModel::new();
        waves.reset(limits);
        self.panels = BTreeMap::from([(id, Panel::new(id, PanelKind::Waves(Box::new(waves))))]);
        self.layout = Layout::single(id);
        self.focused = id;
        if let Some((settings, focus)) = settings {
            self.readd_settings(settings, focus)?;
        }
        Ok(())
    }
}
