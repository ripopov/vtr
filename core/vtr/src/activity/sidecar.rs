//! Where a trace's index lives, and writing one safely.

use super::{Identity, Index, Summary};
use crate::error::Result;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// The places a trace's activity index may live: `<trace>.index` beside the
/// trace (`run.vtr.index`), then a file named by the trace's identity in a
/// cache directory, used when the trace's directory is read-only. A copy of
/// the trace elsewhere finds the same cached index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sidecar {
    pub beside: PathBuf,
    pub cached: Option<PathBuf>,
}

impl Sidecar {
    /// The locations for the trace at `path` with identity `id`; `cache_dir`
    /// is usually [`default_cache_dir`].
    pub fn new(trace: &Path, id: &Identity, cache_dir: Option<&Path>) -> Sidecar {
        let mut beside = trace.as_os_str().to_owned();
        beside.push(".index");
        let cached = cache_dir.map(|d| d.join(format!("{}-{}-{:08x}.index", id.format.name(), id.length, id.toc_crc)));
        Sidecar { beside: beside.into(), cached }
    }

    /// Both locations, beside first.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.beside.as_path()).chain(self.cached.as_deref())
    }

    /// The first index valid for the trace, with its path. Missing, damaged
    /// and stale files are passed over: they are rebuilt, never trusted.
    pub fn load(&self, id: &Identity) -> Option<(PathBuf, Index)> {
        self.paths().find_map(|p| Index::open(p, id).ok().map(|i| (p.to_path_buf(), i)))
    }

    /// Writes an index through `build`, which receives the output stream,
    /// beside the trace or, when its directory refuses the file, in the
    /// cache. The index goes to `<path>.tmp` and is renamed into place only
    /// when complete, so an interrupted build never leaves a file a reader
    /// would load; a failed one removes its temporary file.
    pub fn write(&self, build: impl FnOnce(&mut dyn Write) -> Result<Summary>) -> Result<(PathBuf, Summary)> {
        let (path, file, tmp) = match create_tmp(&self.beside) {
            Ok((file, tmp)) => (self.beside.clone(), file, tmp),
            Err(e) => {
                let Some(cached) = &self.cached else { return Err(e.into()) };
                if let Some(dir) = cached.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let (file, tmp) = create_tmp(cached)?;
                (cached.clone(), file, tmp)
            }
        };
        let done = (|| {
            let mut w = BufWriter::new(file);
            let summary = build(&mut w)?;
            w.into_inner().map_err(|e| e.into_error())?;
            std::fs::rename(&tmp, &path)?;
            Ok(summary)
        })();
        match done {
            Ok(summary) => Ok((path, summary)),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

fn create_tmp(path: &Path) -> std::io::Result<(std::fs::File, PathBuf)> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    Ok((std::fs::File::create(&tmp)?, tmp))
}

/// The user's cache directory for activity indexes: `$XDG_CACHE_HOME/vtr/index`,
/// else `~/Library/Caches/vtr/index` on macOS, `%LOCALAPPDATA%\vtr\index` on
/// Windows and `~/.cache/vtr/index` elsewhere; `None` when the environment
/// names no home.
pub fn default_cache_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if let Some(x) = var("XDG_CACHE_HOME") {
        x
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Caches")
    } else if cfg!(windows) {
        var("LOCALAPPDATA")?
    } else {
        var("HOME")?.join(".cache")
    };
    Some(base.join("vtr").join("index"))
}
