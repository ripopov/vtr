//! Recent traces and workspaces (`volna/volna/ARCHITECTURE.md`, "Workspace persistence"): the list the
//! start page, File ▸ Open Recent and the palette offer. The core owns the
//! list, its order and limit, the start page's selection and keys, and the
//! row text; hosts stamp opens with the wall clock, report which files are
//! missing, persist `state.json` and perform the opens.
//!
//! The list is application state, not part of a session: nothing here is an
//! undo step.
use crate::{App, Event};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Rows the start page shows before *Show all*.
pub const SHOWN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecentKind {
    Trace,
    Workspace,
}

/// One opened trace or workspace file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub uri: String,
    pub kind: RecentKind,
    /// The trace a workspace entry opened, as a URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
    /// Unix seconds of the last open; 0 when unknown.
    pub opened: u64,
}

/// A command from the start page, File ▸ Open Recent or the palette.
/// Indices count from the top of the whole list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecentCommand {
    Open(usize),
    Select(usize),
    Forget(usize),
    Clear,
    /// *Show all* / *Show fewer*.
    ToggleAll,
    Key(RecentKey),
}

/// Keys the start page reads while no trace is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecentKey {
    Up,
    Down,
    Home,
    End,
    Enter,
    Delete,
    /// `1`–`9`: open the shown row with this number.
    Digit(u8),
}

/// The start page's view of the list.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// Hosts that can reopen files by path turn the list on.
    pub enabled: bool,
    pub selected: usize,
    pub expanded: bool,
    /// Entries whose file the host could not find.
    pub missing: BTreeSet<String>,
    /// Bumped whenever the list changes, so hosts can rebuild menus and
    /// recheck files.
    pub revision: u64,
}

/// One row as the start page, menu and palette print it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentRow {
    pub kind: RecentKind,
    /// File name; a workspace drops `.volna.json`.
    pub name: String,
    /// The trace file a workspace opens.
    pub trace: Option<String>,
    /// Parent folder, home as `~`, long paths cut in the middle.
    pub folder: String,
    /// The full path, for tooltips.
    pub path: String,
    /// `12 min ago`, `yesterday`, `Sep 12`, `Not found`, or empty when the
    /// open time is unknown.
    pub when: String,
    pub missing: bool,
}

/// Folders longer than this keep their root and last two folders.
const FOLDER_CHARS: usize = 40;

/// The decoded path of a file URI, `/`-separated.
pub fn uri_path(uri: &str) -> String {
    let Ok(url) = url::Url::parse(uri) else {
        return uri.to_owned();
    };
    let path = percent_encoding::percent_decode_str(url.path())
        .decode_utf8_lossy()
        .into_owned();
    // `file:///C:/x` is a Windows drive path.
    match path.as_bytes() {
        [b'/', _, b':', ..] => path[1..].to_owned(),
        _ => path,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The parent folder of `path`, `home` shown as `~`, and a long one cut in
/// the middle: `~/…/gen/c910`.
pub fn folder(path: &str, home: Option<&str>) -> String {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let mut parent = parent.to_owned();
    if let Some(home) = home.map(|h| h.trim_end_matches('/'))
        && !home.is_empty()
        && (parent == home || parent.starts_with(&format!("{home}/")))
    {
        parent = format!("~{}", &parent[home.len()..]);
    }
    if parent.chars().count() <= FOLDER_CHARS {
        return parent;
    }
    let parts: Vec<&str> = parent.split('/').collect();
    if parts.len() > 3 {
        return [
            parts[0],
            "…",
            parts[parts.len() - 2],
            parts[parts.len() - 1],
        ]
        .join("/");
    }
    let tail: String = {
        let chars: Vec<char> = parent.chars().collect();
        chars[chars.len() - (FOLDER_CHARS - 1)..].iter().collect()
    };
    format!("…{tail}")
}

/// How long ago `opened` was at `now` (both Unix seconds).
pub fn ago(opened: u64, now: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    let d = now.saturating_sub(opened);
    match d {
        _ if d < MIN => "just now".into(),
        _ if d < HOUR => format!("{} min ago", d / MIN),
        _ if d < DAY => format!("{} h ago", d / HOUR),
        _ if d < 2 * DAY => "yesterday".into(),
        _ if d < 7 * DAY => format!("{} days ago", d / DAY),
        _ => {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let (y, m, day) = civil((opened / DAY) as i64);
            let (now_y, _, _) = civil((now / DAY) as i64);
            let month = MONTHS[m as usize - 1];
            if y == now_y {
                format!("{month} {day}")
            } else {
                format!("{month} {day}, {y}")
            }
        }
    }
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (UTC).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Print one entry.
pub fn row(entry: &Recent, missing: bool, now: u64, home: Option<&str>) -> RecentRow {
    let path = uri_path(&entry.uri);
    let name = file_name(&path);
    let name = match entry.kind {
        RecentKind::Workspace => name.strip_suffix(".volna.json").unwrap_or(name),
        RecentKind::Trace => name,
    };
    RecentRow {
        kind: entry.kind,
        name: name.to_owned(),
        trace: entry
            .trace
            .as_deref()
            .map(|t| file_name(&uri_path(t)).to_owned()),
        folder: folder(&path, home),
        when: match (missing, entry.opened) {
            (true, _) => "Not found".into(),
            (false, 0) => String::new(),
            (false, opened) => ago(opened, now),
        },
        missing,
        path,
    }
}

impl App {
    /// The list as rows, newest first.
    pub fn recent_rows(&self, now: u64, home: Option<&str>) -> Vec<RecentRow> {
        let view = &self.workspace.recent;
        self.workspace
            .state
            .recent
            .iter()
            .map(|e| row(e, view.missing.contains(&e.uri), now, home))
            .collect()
    }

    /// Rows the start page shows: the first [`SHOWN`], or all when expanded.
    pub fn recent_shown(&self) -> usize {
        let len = self.recent_len();
        if self.workspace.recent.expanded {
            len
        } else {
            len.min(SHOWN)
        }
    }

    fn recent_len(&self) -> usize {
        if self.workspace.recent.enabled {
            self.workspace.state.recent.len()
        } else {
            0
        }
    }

    /// Show the list (native hosts, which can reopen files by path).
    pub fn enable_recent(&mut self) {
        self.workspace.recent.enabled = true;
        self.recent_changed(false);
    }

    /// Record an open at the top of the list, keeping `workspace.recentLimit`
    /// entries; the host then saves `state.json`.
    pub fn remember_recent(&mut self, entry: Recent) {
        let limit = self.settings.resolved().workspace.recent_limit.max(1);
        let list = &mut self.workspace.state.recent;
        list.retain(|e| e.uri != entry.uri);
        self.workspace.recent.missing.remove(&entry.uri);
        list.insert(0, entry);
        list.truncate(limit);
        self.workspace.recent.selected = 0;
        self.recent_changed(true);
    }

    /// The host checked the files: these are missing.
    pub fn set_recent_missing(&mut self, missing: BTreeSet<String>) {
        if self.workspace.recent.missing != missing {
            self.workspace.recent.missing = missing;
            self.changed();
        }
    }

    pub(crate) fn recent_command(&mut self, command: RecentCommand) {
        let len = self.recent_len();
        let view = &mut self.workspace.recent;
        match command {
            RecentCommand::Open(ix) => {
                if let Some(entry) = self.workspace.state.recent.get(ix).filter(|_| ix < len) {
                    view.selected = ix;
                    self.events.push(Event::OpenRecent(entry.clone()));
                    self.changed();
                }
            }
            RecentCommand::Select(ix) => {
                if ix < self.recent_shown() {
                    self.workspace.recent.selected = ix;
                    self.changed();
                }
            }
            RecentCommand::Forget(ix) => {
                if ix < len {
                    let entry = self.workspace.state.recent.remove(ix);
                    view.missing.remove(&entry.uri);
                    self.recent_changed(true);
                }
            }
            RecentCommand::Clear => {
                if len > 0 {
                    self.workspace.state.recent.clear();
                    view.missing.clear();
                    self.recent_changed(true);
                }
            }
            RecentCommand::ToggleAll => {
                view.expanded = !view.expanded;
                self.recent_changed(false);
            }
            RecentCommand::Key(key) => self.recent_key(key),
        }
    }

    fn recent_key(&mut self, key: RecentKey) {
        let shown = self.recent_shown();
        if shown == 0 {
            return;
        }
        let selected = self.workspace.recent.selected.min(shown - 1);
        let select = match key {
            RecentKey::Up => selected.saturating_sub(1),
            RecentKey::Down => (selected + 1).min(shown - 1),
            RecentKey::Home => 0,
            RecentKey::End => shown - 1,
            RecentKey::Enter => return self.recent_command(RecentCommand::Open(selected)),
            RecentKey::Delete => return self.recent_command(RecentCommand::Forget(selected)),
            RecentKey::Digit(n @ 1..=9) if usize::from(n) <= shown => {
                return self.recent_command(RecentCommand::Open(usize::from(n) - 1));
            }
            RecentKey::Digit(_) => return,
        };
        self.recent_command(RecentCommand::Select(select));
    }

    /// Keep the selection on a shown row and tell the host.
    fn recent_changed(&mut self, persist: bool) {
        let shown = self.recent_shown();
        let view = &mut self.workspace.recent;
        view.selected = view.selected.min(shown.saturating_sub(1));
        view.revision += 1;
        if persist {
            self.events.push(Event::RecentChanged);
        }
        self.changed();
    }

    /// A trace opened or closed: the page starts again on the newest entry.
    pub(crate) fn reset_recent_view(&mut self) {
        self.workspace.recent.selected = 0;
        self.workspace.recent.expanded = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/ada";
    const NOW: u64 = 1_790_589_600; // 2026-09-28 10:00 UTC

    #[test]
    fn folders_shorten_home_and_long_paths() {
        assert_eq!(
            folder("/home/ada/work/scr1/sim/top.vtr", Some(HOME)),
            "~/work/scr1/sim"
        );
        assert_eq!(folder("/home/ada/top.vtr", Some("/home/ada/")), "~");
        assert_eq!(folder("/home/adam/top.vtr", Some(HOME)), "/home/adam");
        assert_eq!(
            folder(
                "/home/ada/work/vtr/bench/workloads/generated/c910/deep/c.vtr",
                Some(HOME)
            ),
            "~/…/c910/deep"
        );
        assert_eq!(folder("/t.vtr", None), "");
    }

    #[test]
    fn times_read_as_ages_then_dates() {
        let at = |secs| ago(NOW - secs, NOW);
        assert_eq!(at(10), "just now");
        assert_eq!(at(12 * 60), "12 min ago");
        assert_eq!(at(2 * 3600 + 300), "2 h ago");
        assert_eq!(at(26 * 3600), "yesterday");
        assert_eq!(at(4 * 86400), "4 days ago");
        assert_eq!(at(9 * 86400), "Sep 19");
        assert_eq!(at(300 * 86400), "Dec 2, 2025");
        assert_eq!(ago(NOW + 50, NOW), "just now", "a clock step back");
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_417), (2023, 3, 1));
    }

    #[test]
    fn rows_name_workspaces_and_decode_uris() {
        let entry = Recent {
            uri: "file:///home/ada/xs/chi%20hang.volna.json".into(),
            kind: RecentKind::Workspace,
            trace: Some("file:///home/ada/xs/dual.vtr".into()),
            opened: NOW - 60,
        };
        let r = row(&entry, false, NOW, Some(HOME));
        assert_eq!(r.name, "chi hang");
        assert_eq!(r.trace.as_deref(), Some("dual.vtr"));
        assert_eq!(r.folder, "~/xs");
        assert_eq!(r.path, "/home/ada/xs/chi hang.volna.json");
        assert_eq!(r.when, "1 min ago");
        assert_eq!(row(&entry, true, NOW, None).when, "Not found");
        assert_eq!(uri_path("file:///C:/sim/top.vtr"), "C:/sim/top.vtr");
    }
}
