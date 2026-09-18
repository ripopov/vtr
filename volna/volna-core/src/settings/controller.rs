//! Settings over the core's command/event loop: GUI edits, the editor tab,
//! host deliveries (file text, watcher changes, overrides, write
//! acknowledgements) and the projection of resolved values into the viewer.
use super::{Host, Store, Value};
use crate::app::{SettingsCommand, SettingsView};
use crate::panels::PanelsCommand;
use crate::{App, Event, Instant};
use std::collections::BTreeMap;

impl App {
    /// Install the store for this host before any document is loaded.
    pub fn configure_settings(&mut self, host: Host, schema_uri: Option<String>) {
        self.settings = Store::new(host);
        self.settings.set_schema_uri(schema_uri);
        let modified = self.settings.modified().to_vec();
        self.apply_settings(&modified);
    }

    /// The text the host read from `settings.json` at start.
    pub fn settings_loaded(&mut self, text: &str) {
        let keys = self.settings.load(text);
        self.settings_changed(keys);
    }

    /// The host could not read `settings.json`; defaults apply and the file
    /// is never overwritten until it can be read again.
    pub fn settings_unreadable(&mut self, error: String) {
        self.settings.mark_unreadable(error.clone());
        self.notice_settings(format!("settings.json is not readable: {error}"));
    }

    /// Bytes reported by the host's file watcher.
    pub fn settings_external(&mut self, bytes: &[u8]) {
        match self.settings.external(bytes) {
            Ok(keys) => self.settings_changed(keys),
            Err(error) => self.notice_settings(format!("settings.json: {error:#}")),
        }
    }

    /// Values a host owns (VS Code's `volna.*` configuration).
    pub fn set_host_settings(&mut self, overrides: BTreeMap<String, Value>) {
        let keys = self.settings.set_overrides(overrides);
        self.settings_changed(keys);
    }

    /// Palette names the host offers for `appearance.theme`.
    pub fn set_theme_names(&mut self, themes: Vec<String>) {
        let keys = self.settings.set_themes(themes);
        self.settings_changed(keys);
    }

    /// The host finished a `WriteSettings` event.
    pub fn settings_saved(&mut self, ticket: u64, error: Option<String>, now: Instant) {
        if !self.settings.acknowledge(ticket, error.clone(), now) {
            return;
        }
        if let Some(error) = error {
            self.notice_settings(format!("settings.json not saved: {error}"));
        }
        self.settings_tick(now, false);
        self.changed();
    }

    /// Emit a pending write once idle; returns true while one is waiting.
    pub(crate) fn settings_tick(&mut self, now: Instant, force: bool) -> bool {
        if let Some((ticket, bytes)) = self.settings.next_write(now, force) {
            self.events.push(Event::WriteSettings { ticket, bytes });
        }
        self.settings.pending_write()
    }

    /// Flush a pending settings write now (quit, window close).
    pub fn flush_settings(&mut self, now: Instant) {
        self.settings_tick(now, true);
    }

    pub(crate) fn settings_command(&mut self, command: SettingsCommand, now: Instant) {
        match command {
            SettingsCommand::Open => {
                match self.panels.open_settings() {
                    Ok(true) => self.layout_changed(),
                    Ok(false) => {}
                    Err(error) => self.events.push(Event::Notice(error.to_string())),
                }
                self.events.push(Event::FocusSettingsSearch);
                self.changed();
            }
            SettingsCommand::Close => {
                if let Some(id) = self.panels.settings_id() {
                    self.handle_at(crate::Command::Panels(PanelsCommand::Close(id)), now);
                }
            }
            SettingsCommand::Set { id, value } => {
                let result = self.settings.set(&id, value, now);
                self.settings_edited(result);
            }
            SettingsCommand::Reset { id } => {
                let result = self.settings.reset(&id, now);
                self.settings_edited(result);
            }
            SettingsCommand::ReplaceText(text) => match self.settings.replace_text(text, now) {
                Ok(keys) => {
                    if let Some(error) = self.settings.syntax_error() {
                        self.notice_settings(format!(
                            "settings.json has a syntax error on line {}; the last good values are kept",
                            error.line
                        ));
                    }
                    self.settings_changed(keys);
                }
                Err(error) => self.notice_settings(error.to_string()),
            },
            SettingsCommand::Query(query) => {
                self.settings_view.query = query;
                self.changed();
            }
            SettingsCommand::ToggleJson => {
                self.settings_view.json = !self.settings_view.json;
                self.changed();
            }
            SettingsCommand::Zoom(step) => {
                let next = step.apply(self.settings.resolved().appearance.zoom);
                let result = if next == 1.0 {
                    self.settings.reset("appearance.zoom", now)
                } else {
                    self.settings
                        .set("appearance.zoom", Value::Number(next), now)
                };
                self.settings_edited(result);
            }
            SettingsCommand::Reveal { id } => {
                self.settings_view = SettingsView {
                    query: format!("@id:{id}"),
                    json: false,
                };
                self.settings_command(SettingsCommand::Open, now);
            }
        }
    }

    fn settings_edited(&mut self, result: anyhow::Result<Vec<&'static str>>) {
        match result {
            Ok(keys) => self.settings_changed(keys),
            Err(error) => self.notice_settings(error.to_string()),
        }
    }

    fn notice_settings(&mut self, text: String) {
        log::warn!("{text}");
        self.events.push(Event::Notice(text));
        self.changed();
    }

    fn settings_changed(&mut self, keys: Vec<&'static str>) {
        self.changed();
        if keys.is_empty() {
            return;
        }
        self.apply_settings(&keys);
        self.events.push(Event::SettingsChanged { keys });
    }

    /// Project resolved values the core consumes itself.
    fn apply_settings(&mut self, keys: &[&'static str]) {
        let settings = self.settings.resolved();
        if keys.contains(&"waves.animation") || keys.contains(&"waves.snapPixels") {
            self.doc.navigation = crate::document::Navigation {
                animation: settings.waves.animation,
                snap_px: f64::from(settings.waves.snap_pixels),
            };
        }
    }
}
