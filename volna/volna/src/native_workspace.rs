//! Native workspace and settings byte storage. File interpretation and target
//! selection live in volna-core; writes use a temporary file in the
//! destination directory. The config directory holds `settings.json`,
//! `state.json`, the generated `settings.schema.json` and `themes/*.json`.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use volna_core::settings::{self, Host};
use volna_core::workspace::{
    MAX_BYTES,
    persistence::{Candidate, Content, Persistence, SaveTicket, Target, hash},
    state::State,
};

pub const SETTINGS_FILE: &str = "settings.json";
pub const STATE_FILE: &str = "state.json";
pub const SCHEMA_FILE: &str = "settings.schema.json";
pub const THEMES_DIR: &str = "themes";

pub struct Store {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    state_writable: bool,
}
impl Store {
    pub fn new(config_dir: Option<PathBuf>) -> Result<Self> {
        let home = || {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME is unavailable")
        };
        let config_dir =
            match config_dir.or_else(|| std::env::var_os("VOLNA_CONFIG_DIR").map(PathBuf::from)) {
                Some(path) => path,
                None => {
                    #[cfg(target_os = "windows")]
                    let root = std::env::var_os("APPDATA")
                        .map(PathBuf::from)
                        .context("APPDATA is unavailable")?;
                    #[cfg(not(target_os = "windows"))]
                    let root = std::env::var_os("XDG_CONFIG_HOME")
                        .map(PathBuf::from)
                        .unwrap_or(home()?.join(".config"));
                    root.join("volna")
                }
            };
        #[cfg(target_os = "macos")]
        let data_dir = home()?.join("Library/Application Support/volna/workspaces");
        #[cfg(target_os = "windows")]
        let data_dir =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?)
                .join("volna/workspaces");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let data_dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".local/share"))
            .join("volna/workspaces");
        Ok(Self {
            config_dir,
            data_dir,
            state_writable: true,
        })
    }

    pub fn settings_path(&self) -> PathBuf {
        self.config_dir.join(SETTINGS_FILE)
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir.join(THEMES_DIR)
    }

    /// The `$schema` value a fresh document points at; the schema file is
    /// regenerated beside it on every start so external editors stay current.
    pub fn schema_uri() -> String {
        format!("./{SCHEMA_FILE}")
    }

    /// Move a version 1 `preferences.json` into `settings.json` and
    /// `state.json` once; the old file is renamed afterwards.
    pub fn migrate_preferences(&self) -> Result<bool> {
        let old = self.config_dir.join("preferences.json");
        if !old.exists() || self.settings_path().exists() {
            return Ok(false);
        }
        let bytes = read_limited(&old, settings::store::MAX_BYTES)?.unwrap_or_default();
        let migrated = settings::migrate::preferences_v1(&bytes, Some(&Self::schema_uri()))?;
        std::fs::create_dir_all(&self.config_dir)?;
        atomic_write(&self.settings_path(), migrated.settings.as_bytes())?;
        if !self.config_dir.join(STATE_FILE).exists() {
            atomic_write(
                &self.config_dir.join(STATE_FILE),
                &migrated.state.to_bytes(),
            )?;
        }
        let mut renamed = old.clone().into_os_string();
        renamed.push(".migrated");
        std::fs::rename(&old, renamed)?;
        Ok(true)
    }

    /// `settings.json` text, `None` when absent. Errors are reported to the
    /// core, which then never writes the file.
    pub fn read_settings(&self) -> Result<Option<String>> {
        match read_limited(&self.settings_path(), settings::store::MAX_BYTES)? {
            Some(bytes) => Ok(Some(
                String::from_utf8(bytes).context("settings.json is not UTF-8")?,
            )),
            None => Ok(None),
        }
    }

    pub fn write_settings(&self, bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        atomic_write(&self.settings_path(), bytes)
    }

    /// Regenerate the schema external editors read through `$schema`.
    pub fn write_schema(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        let schema = serde_json::to_vec_pretty(&settings::schema::json_schema(Host::Native))?;
        let path = self.config_dir.join(SCHEMA_FILE);
        if std::fs::read(&path).ok().as_deref() == Some(schema.as_slice()) {
            return Ok(());
        }
        atomic_write(&path, &schema)
    }

    /// Recent lists. An unreadable or incompatible file yields empty lists and
    /// disables writes so the file is never destroyed.
    pub fn state(&mut self) -> State {
        let path = self.config_dir.join(STATE_FILE);
        match read_limited(&path, volna_core::workspace::state::MAX_BYTES)
            .and_then(|bytes| bytes.map(|b| State::parse(&b)).transpose())
        {
            Ok(Some(state)) => state,
            Ok(None) => State::new(),
            Err(error) => {
                log::warn!("state.json: {error:#}");
                self.state_writable = false;
                State::new()
            }
        }
    }

    pub fn save_state(&self, state: &State) -> Result<()> {
        ensure!(
            self.state_writable,
            "state.json not overwritten after a read or format error"
        );
        std::fs::create_dir_all(&self.config_dir)?;
        atomic_write(&self.config_dir.join(STATE_FILE), &state.to_bytes())
    }

    /// Palette names: the stems of `themes/*.json`, sorted.
    pub fn theme_names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.themes_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension().is_some_and(|e| e == "json"))
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Resolve `appearance.theme` to a core theme; `one-dark` needs no file.
    pub fn theme(&self, name: &str) -> Result<volna_core::Theme> {
        if name == "one-dark" {
            return Ok(volna_core::Theme::one_dark());
        }
        let path = self.themes_dir().join(format!("{name}.json"));
        let bytes = read_limited(&path, settings::store::MAX_BYTES)?
            .with_context(|| format!("theme file does not exist: {}", path.display()))?;
        let json = String::from_utf8(bytes).context("theme file is not UTF-8")?;
        let palette = volna_core::theme::HostPalette::from_json(&json)
            .with_context(|| format!("invalid palette {}", path.display()))?;
        Ok(volna_core::Theme::from_host(&palette))
    }

    /// Watch the config directory (settings and themes). The callback runs on
    /// the watcher thread with the paths that changed.
    pub fn watch(
        &self,
        callback: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> Result<notify::RecommendedWatcher> {
        use notify::Watcher;
        std::fs::create_dir_all(&self.config_dir)?;
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // Our own reads report access events; only content changes
                // count, or every reload would trigger the next one.
                use notify::EventKind;
                if let Ok(event) = event
                    && matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    )
                    && !event.paths.is_empty()
                {
                    callback(event.paths);
                }
            })?;
        watcher.watch(&self.config_dir, notify::RecursiveMode::Recursive)?;
        Ok(watcher)
    }

    pub fn candidates(
        &self,
        trace_uri: &str,
        policy: &Persistence,
    ) -> Result<(Candidate, Candidate)> {
        ensure!(*policy != Persistence::Disabled, "persistence is disabled");
        let trace = path_from_uri(trace_uri)?;
        let mut name = trace.as_os_str().to_owned();
        name.push(".volna.json");
        let target = match policy {
            Persistence::Explicit(target) => target.clone(),
            _ => Target::File {
                uri: file_uri(Path::new(&name))?,
            },
        };
        let fallback_path = self.data_dir.join(format!(
            "{}.volna.json",
            &hash(trace.to_string_lossy().as_bytes())[..16]
        ));
        let fallback = Target::FallbackFile {
            uri: file_uri(&fallback_path)?,
        };
        let side = candidate(target, true);
        let back = if matches!(policy, Persistence::Explicit(_)) {
            Candidate {
                target: fallback,
                content: Content::Missing,
                writable: true,
            }
        } else {
            candidate(fallback, false)
        };
        Ok((side, back))
    }

    pub fn write(&self, ticket: &SaveTicket, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() <= MAX_BYTES, "workspace exceeds size limit");
        let path = path_from_uri(ticket.target.location())?;
        if matches!(ticket.target, Target::FallbackFile { .. }) {
            std::fs::create_dir_all(path.parent().context("missing parent directory")?)?;
        }
        atomic_write(&path, bytes)
    }
}

fn candidate(target: Target, probe: bool) -> Candidate {
    let path = path_from_uri(target.location());
    let (content, writable) = match path {
        Ok(path) => {
            let content = match read_limited(&path, MAX_BYTES) {
                Ok(Some(bytes)) => Content::Bytes(bytes),
                Ok(None) => Content::Missing,
                Err(error) => Content::Error(format!("{error:#}")),
            };
            let writable = !probe
                || path
                    .parent()
                    .is_some_and(|parent| tempfile::NamedTempFile::new_in(parent).is_ok());
            (content, writable)
        }
        Err(error) => (Content::Error(error.to_string()), false),
    };
    Candidate {
        target,
        content,
        writable,
    }
}

pub fn read_limited(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "file exceeds size limit: {}",
        path.display()
    );
    Ok(Some(bytes))
}

pub fn file_uri(path: &Path) -> Result<String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    url::Url::from_file_path(absolute)
        .map(Into::into)
        .map_err(|_| anyhow::anyhow!("invalid file path"))
}
pub fn path_from_uri(uri: &str) -> Result<PathBuf> {
    url::Url::parse(uri)?
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("not a local file URI: {uri}"))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("missing parent directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

impl crate::Workspace {
    /// Install the byte store: migrate and read `settings.json`, read
    /// `state.json`, derive the workspace policy and, when `watch` is set,
    /// follow the config directory (tests keep their scheduler on one
    /// thread). `policy` is the command line's choice; `Auto` follows
    /// `workspace.autosave`.
    pub fn enable_native_persistence(
        &mut self,
        mut store: Store,
        policy: Persistence,
        watch: bool,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        self.app
            .configure_settings(Host::Native, Some(Store::schema_uri()));
        match store.migrate_preferences() {
            Ok(true) => log::info!("migrated preferences.json to settings.json and state.json"),
            Ok(false) => {}
            Err(error) => log::warn!("preferences migration: {error:#}"),
        }
        if let Err(error) = store.write_schema() {
            log::warn!("settings schema: {error:#}");
        }
        self.app.set_theme_names(store.theme_names());
        match store.read_settings() {
            Ok(Some(text)) => self.app.settings_loaded(&text),
            Ok(None) => {}
            Err(error) => self.app.settings_unreadable(format!("{error:#}")),
        }
        self.app.workspace.state = store.state();
        let policy = match policy {
            Persistence::Auto
                if self.app.settings.resolved().workspace.autosave
                    == volna_core::settings::Autosave::Off =>
            {
                Persistence::Disabled
            }
            policy => policy,
        };
        log::debug!("settings loaded from {}", store.settings_path().display());
        self.cli_policy = matches!(policy, Persistence::Explicit(_));
        self.app.configure_persistence(policy);
        if watch {
            self.start_config_watcher(&store, cx);
        }
        self.native_store = Some(store);
        self.apply_theme_setting(cx);
        self.after(None, cx);
    }

    fn start_config_watcher(&mut self, store: &Store, cx: &mut gpui_kit::Context<Self>) {
        use futures::StreamExt;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Vec<PathBuf>>();
        match store.watch(move |paths| {
            tx.unbounded_send(paths).ok();
        }) {
            Ok(watcher) => self.config_watcher = Some(watcher),
            Err(error) => {
                log::warn!("settings watcher: {error:#}");
                return;
            }
        }
        cx.spawn(async move |this, cx| {
            while let Some(mut paths) = rx.next().await {
                // Coalesce a burst (temporary file, rename) into one read.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                while let Ok(more) = rx.try_recv() {
                    paths.extend(more);
                }
                if this
                    .update(cx, |this, cx| this.config_changed(&paths, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// The watcher reported changes under the config directory.
    fn config_changed(&mut self, paths: &[PathBuf], cx: &mut gpui_kit::Context<Self>) {
        log::debug!("config directory changed: {paths:?}");
        let Some(store) = &self.native_store else {
            return;
        };
        let settings_path = store.settings_path();
        let themes_dir = store.themes_dir();
        if paths.contains(&settings_path) {
            match read_limited(&settings_path, settings::store::MAX_BYTES) {
                Ok(Some(bytes)) => self.app.settings_external(&bytes),
                Ok(None) => self.app.settings_external(b""),
                Err(error) => self.app.settings_unreadable(format!("{error:#}")),
            }
        }
        if paths.iter().any(|p| p.starts_with(&themes_dir)) {
            let names = store.theme_names();
            self.app.set_theme_names(names);
            // A changed palette file reloads the current theme.
            self.apply_theme_setting(cx);
        }
        self.after(None, cx);
    }

    /// Write `settings.json` for a core write event.
    pub(crate) fn write_settings(&mut self, ticket: u64, bytes: Vec<u8>) {
        let result = self
            .native_store
            .as_ref()
            .context("settings storage not configured")
            .and_then(|store| store.write_settings(&bytes));
        self.app.settings_saved(
            ticket,
            result.err().map(|e| format!("{e:#}")),
            volna_core::Instant::now(),
        );
    }

    fn remember_recent(&mut self, workspace: bool, uri: String) {
        let limit = self.app.settings.resolved().workspace.recent_limit;
        let state = &mut self.app.workspace.state;
        let list = if workspace {
            &mut state.recent_workspaces
        } else {
            &mut state.recent_traces
        };
        State::remember(list, uri, limit);
        if let Some(store) = &self.native_store
            && let Err(error) = store.save_state(&self.app.workspace.state)
        {
            log::warn!("state.json: {error:#}");
        }
    }

    pub(crate) fn load_workspace_candidates(&mut self, trace_uri: String) {
        let Some(store) = &self.native_store else {
            return;
        };
        match store.candidates(&trace_uri, &self.app.workspace.scheduler.policy) {
            Ok((side, back)) => self.app.restore_candidates(&trace_uri, side, back),
            Err(error) => {
                self.app.workspace.scheduler.suspend(error.to_string());
                self.app.report_workspace_error(error.to_string());
            }
        }
        self.remember_recent(false, trace_uri);
    }

    pub(crate) fn write_workspace(&mut self, ticket: SaveTicket, bytes: Vec<u8>) {
        let result = self
            .native_store
            .as_ref()
            .context("workspace storage not configured")
            .and_then(|store| store.write(&ticket, &bytes));
        self.app.workspace_saved(
            ticket,
            result.err().map(|e| format!("{e:#}")),
            volna_core::Instant::now(),
        );
    }

    pub(crate) fn workspace_dialog(&mut self, save: bool, cx: &mut gpui_kit::Context<Self>) {
        if !self.app.workspace.scheduler.enabled() {
            return;
        }
        if save {
            let trace = self
                .app
                .workspace
                .trace_uri
                .as_deref()
                .and_then(|uri| path_from_uri(uri).ok());
            let directory = trace
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(Path::new("."));
            let name = format!(
                "{}.volna.json",
                self.app.doc.name().unwrap_or_else(|| "workspace".into())
            );
            let rx = cx.prompt_for_new_path(directory, Some(&name));
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(path))) = rx.await {
                    this.update(cx, |this, cx| {
                        match file_uri(&path) {
                            Ok(uri) => this.app.save_workspace(Some(Target::File { uri })),
                            Err(error) => this.app.report_workspace_error(error.to_string()),
                        }
                        this.after(None, cx);
                    })
                    .ok();
                }
            })
            .detach();
        } else {
            let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Open Workspace".into()),
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(paths))) = rx.await
                    && let Some(path) = paths.into_iter().next()
                {
                    this.update(cx, |this, cx| this.open_workspace_path(path, cx))
                        .ok();
                }
            })
            .detach();
        }
    }

    pub fn open_workspace_path(&mut self, path: PathBuf, cx: &mut gpui_kit::Context<Self>) {
        let result = (|| {
            ensure!(
                self.app.workspace.scheduler.enabled(),
                "workspace persistence is disabled"
            );
            let bytes = read_limited(&path, MAX_BYTES)?.context("workspace file does not exist")?;
            let uri = file_uri(&path)?;
            let target = Target::File { uri: uri.clone() };
            if self.app.doc.is_loaded() {
                self.app.open_workspace(target, &bytes)?;
            } else {
                let workspace = volna_core::workspace::Workspace::parse(&bytes)?;
                let trace_uri = volna_core::workspace::resolve_trace(&workspace.trace.path, &uri)?;
                let trace = path_from_uri(&trace_uri)?.canonicalize()?;
                self.app
                    .configure_persistence(Persistence::Explicit(target));
                self.app.open_resource(
                    volna_core::session::OpenSpec::Path(trace.clone()),
                    file_uri(&trace)?,
                );
            }
            Ok::<_, anyhow::Error>(uri)
        })();
        let result = result.map(|uri| self.remember_recent(true, uri));
        if let Err(error) = result {
            self.app
                .report_workspace_error(format!("Cannot open workspace: {error:#}"));
        }
        self.after(None, cx);
    }
}

pub struct Options {
    pub file: Option<PathBuf>,
    pub synthetic: Option<usize>,
    pub policy: Persistence,
    pub config_dir: Option<PathBuf>,
    pub help: bool,
}
impl Options {
    /// Command line wins over environment; no-workspace is an unconditional opt-out.
    pub fn parse(
        args: impl IntoIterator<Item = String>,
        workspace_env: Option<String>,
        config_env: Option<PathBuf>,
    ) -> Result<Self> {
        let mut options = Self {
            file: None,
            synthetic: None,
            policy: match workspace_env.as_deref() {
                None => Persistence::Auto,
                Some("off") => Persistence::Disabled,
                Some(path) => Persistence::Explicit(Target::File {
                    uri: file_uri(Path::new(path))?,
                }),
            },
            config_dir: config_env,
            help: false,
        };
        let mut disabled = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-workspace" => disabled = true,
                "--workspace" => {
                    let path = args.next().context("--workspace requires a path")?;
                    options.policy = Persistence::Explicit(Target::File {
                        uri: file_uri(Path::new(&path))?,
                    });
                }
                "--config-dir" => {
                    options.config_dir =
                        Some(args.next().context("--config-dir requires a path")?.into())
                }
                "--synthetic" => {
                    options.synthetic = Some(
                        args.next()
                            .context("--synthetic requires a count")?
                            .replace('_', "")
                            .parse()
                            .context("invalid synthetic count")?,
                    )
                }
                "-h" | "--help" => options.help = true,
                "--" => {
                    if let Some(path) = args.next() {
                        ensure!(options.file.is_none(), "only one input file is supported");
                        options.file = Some(path.into());
                    }
                    ensure!(args.next().is_none(), "only one input file is supported");
                    break;
                }
                _ => {
                    ensure!(!arg.starts_with('-'), "unknown option: {arg}");
                    ensure!(options.file.is_none(), "only one input file is supported");
                    options.file = Some(arg.into());
                }
            }
        }
        if disabled {
            options.policy = Persistence::Disabled;
        }
        ensure!(
            options.file.is_none() || options.synthetic.is_none(),
            "choose a file or --synthetic"
        );
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_overrides_environment_and_opt_out_wins() {
        let args = [
            "--workspace",
            "chosen.json",
            "--no-workspace",
            "--config-dir",
            "/tmp/volna-config",
            "trace.vtr",
        ]
        .map(str::to_owned);
        let options = Options::parse(args, Some("env.json".into()), None).unwrap();
        assert_eq!(options.policy, Persistence::Disabled);
        assert_eq!(options.config_dir, Some(PathBuf::from("/tmp/volna-config")));
        assert_eq!(options.file, Some(PathBuf::from("trace.vtr")));
        let options = Options::parse(
            ["--workspace", "chosen.json"].map(str::to_owned),
            Some("off".into()),
            None,
        )
        .unwrap();
        assert!(matches!(options.policy, Persistence::Explicit(_)));
        for args in [
            vec!["--workspace"],
            vec!["--config-dir"],
            vec!["--synthetic", "bad"],
            vec!["--typo"],
            vec!["a", "b"],
        ] {
            assert!(Options::parse(args.into_iter().map(str::to_owned), None, None).is_err());
        }
    }

    #[test]
    fn atomic_store_keeps_old_bytes_on_failure_and_cleans_temporary_files() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("saved.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let bad_destination = temporary.path().join("directory");
        std::fs::create_dir(&bad_destination).unwrap();
        assert!(atomic_write(&bad_destination, b"cannot replace directory").is_err());
        assert!(bad_destination.is_dir());
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 2);
    }

    #[test]
    fn settings_state_schema_and_themes_live_in_the_config_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::new(Some(temporary.path().join("config"))).unwrap();
        assert_eq!(store.read_settings().unwrap(), None);
        assert!(!store.migrate_preferences().unwrap());
        std::fs::create_dir_all(&store.config_dir).unwrap();
        std::fs::write(
            store.config_dir.join("preferences.json"),
            br#"{"version":1,"autosave":false,"recent_traces":["file:///t.vtr"]}"#,
        )
        .unwrap();
        assert!(store.migrate_preferences().unwrap());
        assert!(!store.migrate_preferences().unwrap());
        assert!(store.config_dir.join("preferences.json.migrated").exists());
        assert!(
            store
                .read_settings()
                .unwrap()
                .unwrap()
                .contains("\"workspace.autosave\": \"off\"")
        );
        assert_eq!(store.state().recent_traces, vec!["file:///t.vtr"]);
        store.write_schema().unwrap();
        let schema: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.config_dir.join(SCHEMA_FILE)).unwrap())
                .unwrap();
        assert!(schema["properties"]["waves.snapPixels"].is_object());
        assert!(store.theme_names().is_empty());
        std::fs::create_dir_all(store.themes_dir()).unwrap();
        std::fs::write(
            store.themes_dir().join("paper.json"),
            b"{\"appearance\": \"light\"}",
        )
        .unwrap();
        std::fs::write(store.themes_dir().join("notes.txt"), b"").unwrap();
        assert_eq!(store.theme_names(), vec!["paper"]);
        assert!(store.theme("paper").unwrap().appearance == volna_core::theme::Appearance::Light);
        assert!(store.theme("missing").is_err());
        store.write_settings(b"{}").unwrap();
        assert_eq!(store.read_settings().unwrap().as_deref(), Some("{}"));
        // A broken state file is never overwritten.
        std::fs::write(store.config_dir.join(STATE_FILE), b"{\"version\":9}").unwrap();
        assert_eq!(store.state(), State::new());
        assert!(store.save_state(&State::new()).is_err());
        assert_eq!(
            std::fs::read(store.config_dir.join(STATE_FILE)).unwrap(),
            b"{\"version\":9}"
        );
    }

    #[test]
    fn fallback_storage_is_isolated_from_trace_and_config_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::new(Some(temporary.path().join("config"))).unwrap();
        store.data_dir = temporary.path().join("data");
        let trace = temporary.path().join("trace.vtr");
        std::fs::write(&trace, b"trace").unwrap();
        let uri = file_uri(&trace).unwrap();
        assert!(store.candidates(&uri, &Persistence::Disabled).is_err());
        let (side, fallback) = store.candidates(&uri, &Persistence::Auto).unwrap();
        assert_eq!(side.target.location(), format!("{uri}.volna.json"));
        assert!(matches!(side.content, Content::Missing));
        assert!(matches!(fallback.target, Target::FallbackFile { .. }));
        assert!(!store.data_dir.exists());
        let mut state = store.state();
        State::remember(&mut state.recent_traces, uri.clone(), 20);
        store.save_state(&state).unwrap();
        assert_eq!(store.state().recent_traces, vec![uri]);
        assert!(!store.config_dir.join(SETTINGS_FILE).exists());
    }
}

#[cfg(test)]
mod contribution_tests {
    /// The extension manifest carries the registry's VS Code block verbatim.
    #[test]
    fn package_json_configuration_matches_the_registry() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/vscode-ext/package.json");
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let expected = volna_core::settings::schema::vscode_configuration();
        let actual = &manifest["contributes"]["configuration"];
        assert!(
            actual == &expected,
            "vscode-ext/package.json contributes.configuration is stale; expected:\n{}",
            serde_json::to_string_pretty(&expected).unwrap()
        );
    }
}
