//! Toolkit-neutral docking tree. External proposals are validated before
//! normalization; malformed trees are never silently repaired on restore.

use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::PanelId;

pub const MAX_LAYOUT_DEPTH: usize = 64;
pub const MAX_PANELS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Tab selection uses stable identity, including in the serialized form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Layout {
    Tabs {
        tabs: Vec<PanelId>,
        active: PanelId,
    },
    Split {
        split: Axis,
        sizes: Vec<f32>,
        children: Vec<Layout>,
    },
}

impl Layout {
    pub fn single(id: PanelId) -> Self {
        Self::Tabs {
            tabs: vec![id],
            active: id,
        }
    }

    /// Layout order, including inactive tabs, for keyboard cycling.
    pub fn panels(&self) -> Vec<PanelId> {
        let mut ids = Vec::new();
        self.collect(false, &mut ids);
        ids
    }

    pub fn visible(&self) -> Vec<PanelId> {
        let mut ids = Vec::new();
        self.collect(true, &mut ids);
        ids
    }

    fn collect(&self, visible: bool, ids: &mut Vec<PanelId>) {
        match self {
            Self::Tabs { active, .. } if visible => ids.push(*active),
            Self::Tabs { tabs, .. } => ids.extend(tabs),
            Self::Split { children, .. } => {
                for child in children {
                    child.collect(visible, ids);
                }
            }
        }
    }

    pub fn validate(&self, expected: &BTreeSet<PanelId>) -> Result<()> {
        ensure!(
            !expected.is_empty() && expected.len() <= MAX_PANELS,
            "invalid panel count"
        );
        let mut seen = BTreeSet::new();
        let mut nodes = 0;
        self.validate_node(0, &mut nodes, &mut seen)?;
        ensure!(
            &seen == expected,
            "layout panel IDs do not match the workspace"
        );
        Ok(())
    }

    fn validate_node(
        &self,
        depth: usize,
        nodes: &mut usize,
        seen: &mut BTreeSet<PanelId>,
    ) -> Result<()> {
        *nodes += 1;
        ensure!(
            depth <= MAX_LAYOUT_DEPTH && *nodes <= MAX_PANELS * 2,
            "layout exceeds structural limits"
        );
        match self {
            Self::Tabs { tabs, active } => {
                ensure!(
                    !tabs.is_empty() && tabs.contains(active),
                    "empty group or invalid active tab"
                );
                for id in tabs {
                    ensure!(id.0 != 0 && seen.insert(*id), "zero or duplicate panel ID");
                }
            }
            Self::Split {
                sizes, children, ..
            } => {
                ensure!(
                    children.len() >= 2 && children.len() == sizes.len(),
                    "invalid split arity"
                );
                ensure!(
                    sizes.iter().all(|s| s.is_finite() && *s > 0.0),
                    "split sizes must be finite and positive"
                );
                for child in children {
                    child.validate_node(depth + 1, nodes, seen)?;
                }
            }
        }
        Ok(())
    }

    /// The active tab in the group containing a particular panel.
    pub fn active_for(&self, id: PanelId) -> Option<PanelId> {
        match self {
            Self::Tabs { tabs, active } => tabs.contains(&id).then_some(*active),
            Self::Split { children, .. } => children.iter().find_map(|child| child.active_for(id)),
        }
    }

    pub(crate) fn group_mut(&mut self, id: PanelId) -> Option<&mut Layout> {
        match self {
            Self::Tabs { tabs, .. } if tabs.contains(&id) => Some(self),
            Self::Split { children, .. } => children.iter_mut().find_map(|c| c.group_mut(id)),
            _ => None,
        }
    }

    pub(crate) fn activate(&mut self, id: PanelId) -> bool {
        if let Some(Self::Tabs { active, .. }) = self.group_mut(id) {
            *active = id;
            true
        } else {
            false
        }
    }

    pub(crate) fn split(&mut self, id: PanelId, new: PanelId, axis: Axis) {
        let group = self.group_mut(id).expect("known panel");
        let old = std::mem::replace(group, Self::single(new));
        *group = Self::Split {
            split: axis,
            sizes: vec![0.5, 0.5],
            children: vec![old, Self::single(new)],
        };
    }

    /// Remove a tab without changing a surviving active tab. Return the
    /// next tab in its own group as the preferred focus successor.
    pub(crate) fn remove(&mut self, id: PanelId) -> Option<PanelId> {
        let Some(Self::Tabs { tabs, active }) = self.group_mut(id) else {
            return None;
        };
        let ix = tabs.iter().position(|p| *p == id).unwrap();
        tabs.remove(ix);
        let next = tabs.get(ix.min(tabs.len().saturating_sub(1))).copied();
        if *active == id
            && let Some(next) = next
        {
            *active = next;
        }
        next
    }

    /// Bottom-up normalization preserves the relative space of surviving
    /// siblings and multiplies shares when flattening same-axis splits.
    pub(crate) fn normalized(self) -> Option<Self> {
        match self {
            Self::Tabs { ref tabs, .. } if tabs.is_empty() => None,
            Self::Tabs { .. } => Some(self),
            Self::Split {
                split,
                sizes,
                children,
            } => {
                let mut out = Vec::new();
                let mut shares = Vec::new();
                for (child, share) in children.into_iter().zip(sizes) {
                    match child.normalized() {
                        None => {}
                        Some(Self::Split {
                            split: axis,
                            sizes,
                            children,
                        }) if split == axis => {
                            shares.extend(sizes.into_iter().map(|s| s * share));
                            out.extend(children);
                        }
                        Some(child) => {
                            shares.push(share);
                            out.push(child);
                        }
                    }
                }
                match out.len() {
                    0 => None,
                    1 => out.pop(),
                    _ => {
                        let total: f64 = shares.iter().map(|s| f64::from(*s)).sum();
                        for share in &mut shares {
                            *share = (f64::from(*share) / total) as f32;
                        }
                        Some(Self::Split {
                            split,
                            sizes: shares,
                            children: out,
                        })
                    }
                }
            }
        }
    }
}
