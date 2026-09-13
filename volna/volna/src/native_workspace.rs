//! Native workspace byte storage. File interpretation and target selection live
//! in volna-core; writes use a temporary file in the destination directory.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use volna_core::workspace::{
    MAX_BYTES,
    persistence::{Candidate, Content, Persistence, SaveTicket, Target, hash},
    prefs::Preferences,
};

pub struct Store {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    preferences_writable: bool,
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
            preferences_writable: true,
        })
    }

    pub fn preferences(&mut self) -> Result<Preferences> {
        let path = self.config_dir.join("preferences.json");
        match read_limited(&path, 1024 * 1024) {
            Ok(Some(bytes)) => {
                Preferences::parse(&bytes).inspect_err(|_| self.preferences_writable = false)
            }
            Ok(None) => Ok(Preferences::default()),
            Err(error) => {
                self.preferences_writable = false;
                Err(error)
            }
        }
    }

    pub fn save_preferences(&self, prefs: &Preferences) -> Result<()> {
        ensure!(
            self.preferences_writable,
            "preferences not overwritten after a read or format error"
        );
        std::fs::create_dir_all(&self.config_dir)?;
        atomic_write(
            &self.config_dir.join("preferences.json"),
            &serde_json::to_vec_pretty(prefs)?,
        )
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
    pub fn enable_native_persistence(
        &mut self,
        store: Store,
        policy: Persistence,
        preferences: Preferences,
    ) {
        self.native_store = Some(store);
        if preferences.theme != "one-dark" {
            self.app.report_workspace_error(format!(
                "Unknown theme '{}'; using One Dark",
                preferences.theme
            ));
        }
        self.app.workspace.preferences = preferences;
        self.app.configure_persistence(policy);
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
        Preferences::remember(&mut self.app.workspace.preferences.recent_traces, trace_uri);
        if let Some(store) = &self.native_store
            && let Err(error) = store.save_preferences(&self.app.workspace.preferences)
        {
            log::warn!("preferences: {error:#}");
        }
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
            Preferences::remember(&mut self.app.workspace.preferences.recent_workspaces, uri);
            Ok::<_, anyhow::Error>(())
        })();
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
    fn preferences_and_fallback_are_isolated_from_trace_and_config_paths() {
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
        let mut prefs = store.preferences().unwrap();
        Preferences::remember(&mut prefs.recent_traces, uri.clone());
        store.save_preferences(&prefs).unwrap();
        assert_eq!(store.preferences().unwrap().recent_traces, vec![uri]);
        std::fs::write(
            store.config_dir.join("preferences.json"),
            b"{\"version\":99}",
        )
        .unwrap();
        assert!(store.preferences().is_err());
        assert!(store.save_preferences(&prefs).is_err());
        assert_eq!(
            std::fs::read(store.config_dir.join("preferences.json")).unwrap(),
            b"{\"version\":99}"
        );
    }
}
