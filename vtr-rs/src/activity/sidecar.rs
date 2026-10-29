//! Where a trace's index lives, and writing one safely.

use super::{BuildControl, Identity, Index, Summary};
use crate::error::Result;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sidecar {
    pub beside: PathBuf,
    pub cached: Option<PathBuf>,
}

impl Sidecar {
    #[must_use]
    pub fn new(trace: &Path, id: &Identity, cache_dir: Option<&Path>) -> Sidecar {
        let mut beside = trace.as_os_str().to_owned();
        beside.push(".index");
        let cached = cache_dir.map(|d| {
            d.join(format!(
                "{}-{}-{}-{:08x}.index",
                id.format.name(),
                id.recording,
                id.length,
                id.toc_crc
            ))
        });
        Sidecar {
            beside: beside.into(),
            cached,
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.beside.as_path()).chain(self.cached.as_deref())
    }

    #[must_use]
    pub fn load(&self, id: &Identity) -> Option<(PathBuf, Index)> {
        self.paths()
            .find_map(|p| Index::open(p, id).ok().map(|i| (p.to_path_buf(), i)))
    }

    pub fn write(
        &self,
        build: impl FnOnce(&mut dyn Write) -> Result<Summary>,
    ) -> Result<(PathBuf, Summary)> {
        self.write_inner(None, build)
    }

    pub fn write_with_control(
        &self,
        control: &BuildControl,
        build: impl FnOnce(&mut dyn Write) -> Result<Summary>,
    ) -> Result<(PathBuf, Summary)> {
        self.write_inner(Some(control), build)
    }

    fn write_inner(
        &self,
        control: Option<&BuildControl>,
        build: impl FnOnce(&mut dyn Write) -> Result<Summary>,
    ) -> Result<(PathBuf, Summary)> {
        if let Some(control) = control {
            control.check()?;
        }
        let (path, file, tmp) = match create_tmp(&self.beside) {
            Ok((file, tmp)) => (self.beside.clone(), file, tmp),
            Err(e) => {
                let Some(cached) = &self.cached else {
                    return Err(e.into());
                };
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
            w.into_inner()
                .map_err(std::io::IntoInnerError::into_error)?;
            let publish = || {
                std::fs::rename(&tmp, &path)?;
                Ok(())
            };
            if let Some(control) = control {
                control.publish(publish)?;
            } else {
                publish()?;
            }
            Ok(summary)
        })();
        match done {
            Ok(summary) => Ok((path, summary)),
            Err(e) => {
                drop(std::fs::remove_file(&tmp));
                Err(e)
            }
        }
    }
}

fn create_tmp(path: &Path) -> std::io::Result<(std::fs::File, PathBuf)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(format!(
            ".tmp.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let tmp = PathBuf::from(tmp);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => return Ok((file, tmp)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

#[must_use]
pub fn default_cache_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
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
