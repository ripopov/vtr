//! Three layers (registry defaults, the user's JSONC document, host
//! overrides) resolved into one [`Settings`] value plus diagnostics, and the
//! write queue that hands the document to the host: one write in flight,
//! acknowledged by ticket, coalesced after a short idle, echoes ignored.

use std::collections::BTreeMap;
use std::time::Duration;

use super::jsonc::{self, Document, SyntaxError};
use super::registry::{Host, META_KEYS, REGISTRY, RENAMED, spec};
use super::{Settings, Value};
use crate::Instant;
use anyhow::{Result, bail, ensure};

/// A change to the document is written after this idle time.
pub const WRITE_IDLE: Duration = Duration::from_millis(300);

/// The settings file is treated as unreadable above this size.
pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A problem in the document. `span` is empty for a syntax error at `offset`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub key: Option<String>,
    /// 1-based.
    pub line: usize,
    pub span: std::ops::Range<usize>,
    pub message: String,
}

pub struct Store {
    host: Host,
    schema_uri: Option<String>,
    themes: Vec<String>,
    text: String,
    parsed: Result<Document, SyntaxError>,
    overrides: BTreeMap<String, Value>,
    values: BTreeMap<&'static str, Value>,
    resolved: Settings,
    diagnostics: Vec<Diagnostic>,
    /// Ids present in the document with a valid value.
    modified: Vec<&'static str>,
    // -- the write queue --
    dirty: bool,
    changed_at: Option<Instant>,
    outstanding: Option<u64>,
    next_ticket: u64,
    last_written: Option<String>,
    last_error: Option<String>,
    /// A read error at start-up makes the file read-only for us.
    writable: bool,
}

impl Store {
    pub fn new(host: Host) -> Self {
        let mut store = Store {
            host,
            schema_uri: None,
            themes: Vec::new(),
            text: String::new(),
            parsed: Ok(Document::default()),
            overrides: BTreeMap::new(),
            values: BTreeMap::new(),
            resolved: Settings::default(),
            diagnostics: Vec::new(),
            modified: Vec::new(),
            dirty: false,
            changed_at: None,
            outstanding: None,
            next_ticket: 1,
            last_written: None,
            last_error: None,
            writable: true,
        };
        store.resolve();
        store
    }

    pub fn host(&self) -> Host {
        self.host
    }

    /// The `$schema` value written into a document created from nothing.
    pub(crate) fn set_schema_uri(&mut self, uri: Option<String>) {
        self.schema_uri = uri;
    }

    /// Palette names the host offers for `appearance.theme`.
    pub(crate) fn set_themes(&mut self, themes: Vec<String>) -> Vec<&'static str> {
        if self.themes == themes {
            return Vec::new();
        }
        self.themes = themes;
        self.resolve()
    }

    pub fn themes(&self) -> &[String] {
        &self.themes
    }

    pub fn resolved(&self) -> &Settings {
        &self.resolved
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn syntax_error(&self) -> Option<&SyntaxError> {
        self.parsed.as_ref().err()
    }

    /// The document parses, so GUI edits are possible.
    pub fn editable(&self) -> bool {
        self.parsed.is_ok() && self.writable
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    fn ensure_writable(&self) -> Result<()> {
        ensure!(
            self.writable,
            "settings.json is not writable after a read error"
        );
        Ok(())
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The resolved generic value of a registered id.
    pub fn value(&self, id: &str) -> Option<&Value> {
        self.values.get(id)
    }

    /// Whether the user's document sets this id.
    pub fn is_modified(&self, id: &str) -> bool {
        self.modified.contains(&id)
    }

    pub fn modified(&self) -> &[&'static str] {
        &self.modified
    }

    /// Whether a host override hides the document value of this id.
    pub fn is_overridden(&self, id: &str) -> bool {
        self.overrides.contains_key(id)
    }

    // -- loading ---------------------------------------------------------------

    /// Install the document the host read at start. An unreadable file is
    /// reported through [`Store::mark_unreadable`] instead.
    pub(crate) fn load(&mut self, text: &str) -> Vec<&'static str> {
        let text = migrate_names(text);
        self.text = text;
        self.parsed = jsonc::parse(&self.text);
        self.resolve()
    }

    /// The host could not read the file: keep defaults, never overwrite it.
    pub(crate) fn mark_unreadable(&mut self, error: String) {
        self.writable = false;
        self.last_error = Some(error);
    }

    /// Values a host owns (VS Code configuration, `--set`). They win over the
    /// document and are never written into it.
    pub(crate) fn set_overrides(
        &mut self,
        overrides: BTreeMap<String, Value>,
    ) -> Vec<&'static str> {
        if self.overrides == overrides {
            return Vec::new();
        }
        self.overrides = overrides;
        self.resolve()
    }

    /// Bytes reported by the host's file watcher. The store's own write comes
    /// back as an echo and is ignored; unchanged text is not a change either.
    pub(crate) fn external(&mut self, bytes: &[u8]) -> Result<Vec<&'static str>> {
        let hash = crate::workspace::persistence::hash(bytes);
        if self.last_written.as_deref() == Some(hash.as_str()) {
            return Ok(Vec::new());
        }
        if bytes.len() > MAX_BYTES {
            bail!("settings file exceeds {MAX_BYTES} bytes");
        }
        let text = std::str::from_utf8(bytes)?;
        if text == self.text {
            return Ok(Vec::new());
        }
        // The file came back; a stale unreadable state no longer applies.
        self.writable = true;
        self.last_error = None;
        Ok(self.load(text))
    }

    // -- edits ---------------------------------------------------------------------

    fn seed(&self) -> Vec<(&str, serde_json::Value)> {
        self.schema_uri
            .as_deref()
            .map(|uri| vec![("$schema", serde_json::Value::String(uri.into()))])
            .unwrap_or_default()
    }

    fn document(&self) -> Result<&Document> {
        match &self.parsed {
            Ok(doc) => Ok(doc),
            Err(error) => bail!(
                "settings.json has a syntax error on line {}: {}",
                error.line,
                error.message
            ),
        }
    }

    /// Set `id` in the document with a surgical edit. Returns the changed keys.
    pub(crate) fn set(
        &mut self,
        id: &str,
        value: Value,
        now: Instant,
    ) -> Result<Vec<&'static str>> {
        let spec = spec(id).ok_or_else(|| anyhow::anyhow!("unknown setting {id}"))?;
        if !spec.available(self.host) {
            bail!("{id} is not available on this host");
        }
        spec.validate(&value, self.host, &self.themes)
            .map_err(|e| anyhow::anyhow!("{id}: {e}"))?;
        self.ensure_writable()?;
        let doc = self.document()?;
        let text = jsonc::set(&self.text, doc, id, &value.to_json(), &self.seed());
        self.commit(text, now)
    }

    /// Remove `id` from the document.
    pub(crate) fn reset(&mut self, id: &str, now: Instant) -> Result<Vec<&'static str>> {
        self.ensure_writable()?;
        let doc = self.document()?;
        if doc.get(id).is_none() {
            return Ok(Vec::new());
        }
        let text = jsonc::remove(&self.text, doc, id);
        self.commit(text, now)
    }

    /// Replace the whole text (the JSON view's save). A syntax error keeps the
    /// last good resolution and is reported as a diagnostic, but the text is
    /// still written so the user's edit is not lost.
    pub(crate) fn replace_text(&mut self, text: String, now: Instant) -> Result<Vec<&'static str>> {
        self.ensure_writable()?;
        if text == self.text {
            return Ok(Vec::new());
        }
        self.commit(text, now)
    }

    fn commit(&mut self, text: String, now: Instant) -> Result<Vec<&'static str>> {
        self.text = text;
        self.parsed = jsonc::parse(&self.text);
        self.dirty = true;
        self.changed_at = Some(now);
        Ok(self.resolve())
    }

    // -- the write queue ----------------------------------------------------------

    /// The bytes to write, once idle (or forced) and nothing is in flight.
    pub(crate) fn next_write(&mut self, now: Instant, force: bool) -> Option<(u64, Vec<u8>)> {
        if !self.dirty || self.outstanding.is_some() || !self.writable {
            return None;
        }
        if !force
            && self
                .changed_at
                .is_none_or(|at| now.saturating_duration_since(at) < WRITE_IDLE)
        {
            return None;
        }
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.outstanding = Some(ticket);
        self.dirty = false;
        let bytes = self.text.clone().into_bytes();
        self.last_written = Some(crate::workspace::persistence::hash(&bytes));
        Some((ticket, bytes))
    }

    /// The host finished the write; a failure keeps the document dirty.
    pub(crate) fn acknowledge(&mut self, ticket: u64, error: Option<String>, now: Instant) -> bool {
        if self.outstanding != Some(ticket) {
            return false;
        }
        self.outstanding = None;
        match error {
            Some(error) => {
                self.last_error = Some(error);
                self.dirty = true;
                self.changed_at = Some(now);
            }
            None => self.last_error = None,
        }
        true
    }

    pub fn pending_write(&self) -> bool {
        self.dirty || self.outstanding.is_some()
    }

    // -- resolution ----------------------------------------------------------------

    /// Diagnostics for text that is not (yet) the document: the JSON view
    /// reports problems while the user types.
    pub fn diagnose(&self, text: &str) -> Vec<Diagnostic> {
        let parsed = jsonc::parse(text);
        self.analyse(&parsed).1
    }

    /// Resolution of one parse: document values (last good values when the
    /// text does not parse), diagnostics, and the ids the document sets.
    #[allow(clippy::type_complexity)]
    fn analyse(
        &self,
        parsed: &Result<Document, SyntaxError>,
    ) -> (
        BTreeMap<&'static str, Value>,
        Vec<Diagnostic>,
        Vec<&'static str>,
    ) {
        let mut values: BTreeMap<&'static str, Value> =
            REGISTRY.iter().map(|s| (s.id, s.default.value())).collect();
        let mut diagnostics = Vec::new();
        let mut modified = Vec::new();
        match parsed {
            Err(error) => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    key: None,
                    line: error.line,
                    span: error.offset..error.offset,
                    message: format!("syntax error: {}", error.message),
                });
                for id in &self.modified {
                    if let Some(value) = self.values.get(id) {
                        values.insert(id, value.clone());
                    }
                }
                modified = self.modified.clone();
            }
            Ok(doc) => {
                let mut seen: Vec<&str> = Vec::new();
                for entry in &doc.entries {
                    if META_KEYS.contains(&entry.key.as_str()) {
                        continue;
                    }
                    if seen.contains(&entry.key.as_str()) {
                        diagnostics.push(warn(
                            entry,
                            format!("duplicate key \"{}\", the last one wins", entry.key),
                        ));
                    }
                    seen.push(&entry.key);
                    let Some(spec) = spec(&entry.key) else {
                        diagnostics.push(warn(entry, format!("unknown setting \"{}\"", entry.key)));
                        continue;
                    };
                    if !spec.available(self.host) {
                        diagnostics.push(warn(
                            entry,
                            format!("\"{}\" is not used on this host", entry.key),
                        ));
                        continue;
                    }
                    let value = json_value(&entry.value);
                    let problem = match &value {
                        Some(value) => spec.validate(value, self.host, &self.themes).err(),
                        None => Some("unsupported value".into()),
                    };
                    if let Some(problem) = problem {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            key: Some(entry.key.clone()),
                            line: entry.line,
                            span: entry.value_span.clone(),
                            message: format!(
                                "\"{}\": {problem}; using default {}",
                                entry.key,
                                spec.default.value().to_json_text()
                            ),
                        });
                        continue;
                    }
                    values.insert(spec.id, value.unwrap());
                    if !modified.contains(&spec.id) {
                        modified.push(spec.id);
                    }
                }
            }
        }
        for (id, value) in &self.overrides {
            if let Some(spec) = spec(id)
                && spec.available(self.host)
                && spec.validate(value, self.host, &self.themes).is_ok()
            {
                values.insert(spec.id, value.clone());
            }
        }
        (values, diagnostics, modified)
    }

    /// Rebuild the resolved struct; returns the ids whose value changed.
    fn resolve(&mut self) -> Vec<&'static str> {
        let (values, diagnostics, modified) = self.analyse(&self.parsed);
        let changed: Vec<&'static str> = values
            .iter()
            .filter(|(id, value)| self.values.get(*id) != Some(value))
            .map(|(id, _)| *id)
            .collect();
        self.resolved = Settings::from_values(&|id| values[id].clone());
        self.values = values;
        self.diagnostics = diagnostics;
        self.modified = modified;
        changed
    }
}

/// Convert a document value into a setting value.
fn json_value(value: &serde_json::Value) -> Option<Value> {
    match value {
        serde_json::Value::Bool(b) => Some(Value::Bool(*b)),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Integer)
            .or_else(|| n.as_f64().map(Value::Number)),
        serde_json::Value::String(s) => Some(Value::Text(s.clone())),
        _ => None,
    }
}

/// Rewrite renamed keys once, keeping a comment with the old name.
fn migrate_names(text: &str) -> String {
    let mut text = text.to_owned();
    for (old, new) in RENAMED {
        let Ok(doc) = jsonc::parse(&text) else { break };
        let Some(entry) = doc.get(old) else { continue };
        if doc.get(new).is_some() {
            continue;
        }
        let key_span = entry.key_span.clone();
        let value_end = entry.value_span.end;
        let mut out = String::new();
        out.push_str(&text[..key_span.start]);
        out.push_str(&format!("\"{new}\""));
        out.push_str(&text[key_span.end..value_end]);
        out.push_str(&format!(" /* was \"{old}\" */"));
        out.push_str(&text[value_end..]);
        text = out;
    }
    text
}

fn warn(entry: &jsonc::Entry, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Warning,
        key: Some(entry.key.clone()),
        line: entry.line,
        span: entry.key_span.clone(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn empty_and_missing_documents_resolve_to_defaults() {
        let mut store = Store::new(Host::Native);
        assert_eq!(store.resolved(), &Settings::default());
        assert!(store.load("").is_empty());
        assert!(store.load("// nothing yet\n").is_empty());
        assert!(store.diagnostics().is_empty());
        let changed = store
            .load("{\"waves.snapPixels\": 9, \"unknown.key\": 1, /* c */ \"$schema\": \"x\",}");
        assert_eq!(changed, vec!["waves.snapPixels"]);
        assert_eq!(store.resolved().waves.snap_pixels, 9);
        assert_eq!(store.modified(), ["waves.snapPixels"]);
        assert_eq!(store.diagnostics().len(), 1);
        assert_eq!(store.diagnostics()[0].severity, Severity::Warning);
    }

    #[test]
    fn invalid_values_fall_back_with_line_numbers() {
        let mut store = Store::new(Host::Native);
        store.load(
            "{\n  \"panels.linkByDefault\": \"yes\",\n  \"waves.snapPixels\": 99,\n  \"waves.animation\": \"fast\",\n  \"workspace.autosave\": \"vscode\"\n}",
        );
        assert_eq!(store.resolved(), &Settings::default());
        let lines: Vec<_> = store.diagnostics().iter().map(|d| d.line).collect();
        assert_eq!(lines, vec![2, 3, 4, 5]);
        assert!(
            store
                .diagnostics()
                .iter()
                .all(|d| d.severity == Severity::Error)
        );
        assert!(store.diagnostics()[1].message.contains("0–24"));
        assert!(store.modified().is_empty());
        assert!(store.editable());
    }

    #[test]
    fn set_and_reset_are_surgical_and_queue_one_write() {
        let mut store = Store::new(Host::Native);
        store.set_schema_uri(Some("./settings.schema.json".into()));
        let text = "{\n  // keep\n  \"waves.animation\": \"reduced\", // why\n}\n";
        store.load(text);
        let t0 = now();
        assert_eq!(
            store.set("waves.snapPixels", 3.into(), t0).unwrap(),
            vec!["waves.snapPixels"]
        );
        assert_eq!(
            store.text(),
            "{\n  // keep\n  \"waves.animation\": \"reduced\", // why\n  \"waves.snapPixels\": 3\n}\n"
        );
        assert!(store.next_write(t0, false).is_none());
        let (ticket, bytes) = store.next_write(t0 + WRITE_IDLE, false).unwrap();
        assert_eq!(bytes, store.text().as_bytes());
        assert!(store.next_write(t0 + WRITE_IDLE, true).is_none());
        assert!(!store.acknowledge(99, None, t0));
        assert!(store.acknowledge(ticket, None, t0));
        assert!(!store.pending_write());
        // The echo of our own write is not a change.
        assert!(store.external(&bytes).unwrap().is_empty());
        assert_eq!(
            store.reset("waves.animation", t0).unwrap(),
            vec!["waves.animation"]
        );
        assert_eq!(store.text(), "{\n  // keep\n  \"waves.snapPixels\": 3\n}\n");
        assert!(store.reset("waves.animation", t0).unwrap().is_empty());
        // A failed write keeps the document dirty.
        let (ticket, _) = store.next_write(t0, true).unwrap();
        store.acknowledge(ticket, Some("disk full".into()), t0);
        assert!(store.pending_write());
        assert_eq!(store.last_error(), Some("disk full"));
        // Values are validated before they reach the document.
        assert!(store.set("waves.snapPixels", 99.into(), t0).is_err());
        assert!(store.set("memory.budgetMiB", 0.into(), t0).is_err());
        assert!(store.set("remote.serverPath", "x".into(), t0).is_err());
        assert!(store.set("nope", true.into(), t0).is_err());
        // A first edit seeds the schema line.
        let mut fresh = Store::new(Host::Native);
        fresh.set_schema_uri(Some("./settings.schema.json".into()));
        fresh.set("panels.linkByDefault", false.into(), t0).unwrap();
        assert_eq!(
            fresh.text(),
            "{\n  \"$schema\": \"./settings.schema.json\",\n  \"panels.linkByDefault\": false\n}\n"
        );
        assert!(fresh.diagnostics().is_empty());
    }

    #[test]
    fn syntax_errors_block_edits_and_keep_the_last_good_values() {
        let mut store = Store::new(Host::Native);
        store.load("{\"waves.snapPixels\": 4}");
        let changed = store
            .replace_text("{\"waves.snapPixels\": 4,,}".into(), now())
            .unwrap();
        assert!(changed.is_empty());
        assert_eq!(
            store.resolved().waves.snap_pixels,
            4,
            "last good values while broken"
        );
        assert_eq!(store.modified(), ["waves.snapPixels"]);
        assert!(!store.editable());
        assert_eq!(store.diagnostics()[0].line, 1);
        assert!(store.set("waves.snapPixels", 5.into(), now()).is_err());
        assert!(store.syntax_error().is_some());
        let changed = store
            .replace_text("{\"waves.snapPixels\": 5}".into(), now())
            .unwrap();
        assert_eq!(changed, vec!["waves.snapPixels"]);
        assert!(store.editable());
    }

    #[test]
    fn external_changes_report_only_changed_keys_and_overrides_win() {
        let mut store = Store::new(Host::Vscode);
        store.load("{\"waves.snapPixels\": 4, \"waves.animation\": \"off\"}");
        let changed = store
            .external(b"{\"waves.snapPixels\": 4, \"waves.animation\": \"reduced\"}")
            .unwrap();
        assert_eq!(changed, vec!["waves.animation"]);
        let mut overrides = BTreeMap::new();
        overrides.insert("waves.snapPixels".to_owned(), Value::Integer(2));
        overrides.insert("memory.budgetMiB".to_owned(), Value::Integer(64));
        overrides.insert("bogus".to_owned(), Value::Integer(64));
        let changed = store.set_overrides(overrides.clone());
        assert_eq!(changed, vec!["memory.budgetMiB", "waves.snapPixels"]);
        assert_eq!(store.resolved().waves.snap_pixels, 2);
        assert!(store.is_overridden("waves.snapPixels"));
        assert!(store.set_overrides(overrides).is_empty());
        assert!(store.external(&vec![b'x'; MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn renamed_keys_are_rewritten_once_with_a_comment() {
        let mut store = Store::new(Host::Vscode);
        store.load("{\n  \"serverPath\": \"/opt/volna-server\"\n}");
        assert_eq!(
            store.text(),
            "{\n  \"remote.serverPath\": \"/opt/volna-server\" /* was \"serverPath\" */\n}"
        );
        assert_eq!(store.resolved().remote.server_path, "/opt/volna-server");
        assert!(store.diagnostics().is_empty());
    }

    #[test]
    fn unreadable_files_are_never_overwritten() {
        let mut store = Store::new(Host::Native);
        store.mark_unreadable("permission denied".into());
        assert!(store.set("waves.snapPixels", 3.into(), now()).is_err());
        assert!(!store.editable());
        assert!(store.external(b"{}").unwrap().is_empty());
        assert!(store.editable());
    }
}
