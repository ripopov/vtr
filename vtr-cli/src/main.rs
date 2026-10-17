use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use vtr::{Reader, Writer};
mod inspect;
mod progress;

#[derive(Parser)]
#[command(
    name = "vtr",
    version,
    about = "Inspect, query, and convert hardware traces",
    after_help = "Times are unsigned integer ticks in the recording's time base. Paths use dots as separators. Exit codes: 0 success, 1 missing index, 2 error, 3 recovery dropped bytes."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Window {
    #[arg(long, default_value_t = 0)]
    from: u64,
    #[arg(long, default_value_t = u64::MAX)]
    to: u64,
}
impl Window {
    fn checked(&self) -> Result<(u64, u64)> {
        ensure!(self.from <= self.to, "--from must not exceed --to");
        Ok((self.from, self.to))
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Kanata,
    Vcd,
    Fst,
}
#[derive(Subcommand)]
enum Command {
    Info {
        file: PathBuf,
    },
    Hierarchy {
        file: PathBuf,
        #[arg(long, default_value_t = usize::MAX)]
        depth: usize,
        #[arg(long)]
        vars: bool,
        #[arg(long)]
        sizes: bool,
    },
    Value {
        file: PathBuf,
        signal: String,
        time: u64,
    },
    Changes {
        file: PathBuf,
        signal: String,
        #[command(flatten)]
        window: Window,
        #[arg(long, default_value_t = 100)]
        max: usize,
    },
    Transactions {
        file: PathBuf,
        #[arg(long, conflicts_with = "id")]
        scope: Option<String>,
        #[command(flatten)]
        window: Window,
        #[arg(long, default_value_t = 100)]
        max: usize,
        #[arg(long, value_delimiter = ',', conflicts_with_all = ["from", "to", "max"])]
        id: Vec<u64>,
    },
    Logs {
        file: PathBuf,
        #[arg(long)]
        scope: Option<String>,
        #[command(flatten)]
        window: Window,
        #[arg(long, default_value_t = 100)]
        max: usize,
        #[arg(long, default_value = "trace", value_parser = parse_severity)]
        severity: vtr::Severity,
        #[arg(long, conflicts_with_all = ["scope", "from", "to", "max", "severity"])]
        sites: bool,
    },
    Clocks {
        file: PathBuf,
    },
    Convert {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, value_enum)]
        format: Option<Format>,
        #[arg(long, value_enum, default_value_t = progress::Style::Auto)]
        progress: progress::Style,
    },
    ToVcd {
        input: PathBuf,
        output: PathBuf,
    },
    Recover {
        input: PathBuf,
        output: PathBuf,
    },
    Index {
        file: PathBuf,
        #[arg(long, conflicts_with_all = ["threads", "memory"])]
        check: bool,
        #[arg(long, default_value_t = 0)]
        threads: usize,
        #[arg(long, default_value_t = 256)]
        memory: u64,
    },
    Active {
        file: PathBuf,
        #[command(flatten)]
        window: Window,
        #[arg(long)]
        build: bool,
    },
}
fn parse_severity(s: &str) -> Result<vtr::Severity, String> {
    vtr::Severity::from_name(s)
        .ok_or_else(|| "expected trace, debug, info, warn, error, or fatal".into())
}

fn output_file<T>(output: &Path, write: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    ensure!(
        !output.try_exists()? && std::fs::symlink_metadata(output).is_err(),
        "output {} already exists; choose a new path",
        output.display()
    );
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let result = write(temp.path())?;
    temp.persist_noclobber(output)
        .with_context(|| format!("cannot publish {}", output.display()))?;
    Ok(result)
}
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "The filename is lowercased before matching compound extensions."
)]
fn format_of(path: &Path) -> Result<Format> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let name = name.strip_suffix(".gz").unwrap_or(&name);
    if name.ends_with(".kanata") || name.ends_with(".log") {
        Ok(Format::Kanata)
    } else if name.ends_with(".vcd") {
        Ok(Format::Vcd)
    } else if name.ends_with(".fst") {
        Ok(Format::Fst)
    } else {
        bail!("unknown input format; use --format kanata, vcd, or fst")
    }
}
fn activity_index(
    path: &Path,
    reader: &Reader,
    build: Option<(usize, u64)>,
) -> Result<Option<vtr::activity::Index>> {
    let options = build
        .map(|(threads, memory)| {
            let memory = memory
                .checked_mul(1 << 20)
                .context("--memory is too large")?;
            ensure!(memory > 0, "--memory must be positive");
            Ok::<_, anyhow::Error>(vtr::activity::BuildOptions {
                threads,
                memory,
                ..Default::default()
            })
        })
        .transpose()?;
    let identity = vtr::activity::Identity::of(reader)?;
    let sidecar = vtr::activity::Sidecar::new(
        path,
        &identity,
        vtr::activity::default_cache_dir().as_deref(),
    );
    let Some(options) = options else {
        return Ok(sidecar.load(&identity).map(|(_, index)| index));
    };
    let lock = sidecar
        .paths()
        .find_map(|path| {
            let mut name = path.as_os_str().to_owned();
            name.push(".lock");
            if let Some(parent) = Path::new(&name).parent() {
                drop(std::fs::create_dir_all(parent));
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(name)
                .ok()
        })
        .context("cannot lock the activity sidecar")?;
    lock.lock()?;
    if let Some((_, index)) = sidecar.load(&identity) {
        return Ok(Some(index));
    }

    let (path, _) = sidecar.write(|out| vtr::activity::build(reader, out, &options))?;
    Ok(Some(vtr::activity::Index::open(path, &identity)?))
}
fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Command::Info { file } => inspect::info(&file)?,
        Command::Hierarchy {
            file,
            depth,
            vars,
            sizes,
        } => inspect::hierarchy(&file, depth, vars, sizes)?,
        Command::Value { file, signal, time } => {
            let r = Reader::open(file)?;
            let id = r
                .find_signal(&signal, '.')
                .with_context(|| format!("signal {signal:?} not found"))?;
            println!("{}", r.value_at(id, time)?.to_ascii());
        }
        Command::Changes {
            file,
            signal,
            window,
            max,
        } => {
            let (from, to) = window.checked()?;
            let r = Reader::open(file)?;
            let id = r
                .find_signal(&signal, '.')
                .with_context(|| format!("signal {signal:?} not found"))?;
            let changes = r.changes(id, from, to)?;
            for (t, value) in changes.iter().take(max) {
                println!("{t}\t{}", value.to_ascii());
            }
            if changes.len() > max {
                eprintln!(
                    "{} more changes; increase --max to show them",
                    changes.len() - max
                );
            }
        }
        Command::Transactions {
            file,
            scope,
            window,
            max,
            id,
        } => inspect::transactions(&file, scope.as_deref(), window.checked()?, max, &id)?,
        Command::Logs {
            file,
            scope,
            window,
            max,
            severity,
            sites,
        } => inspect::logs(
            &file,
            scope.as_deref(),
            window.checked()?,
            max,
            severity,
            sites,
        )?,
        Command::Clocks { file } => inspect::clocks(&file)?,
        Command::Convert {
            input,
            output,
            format,
            progress,
        } => {
            let format = format.map_or_else(|| format_of(&input), Ok)?;
            let mut display =
                progress::Display::start(std::fs::metadata(&input)?.len(), &output, progress);
            output_file(&output, |path| {
                display.output(path);
                let mut writer = Writer::create_with(
                    path,
                    vtr::WriterOptions {
                        dedup: false,
                        ..Default::default()
                    },
                )?;
                match format {
                    Format::Kanata => {
                        vtr_convert::kanata::convert_kanata(&input, &mut writer, &display.reporter)
                    }
                    Format::Vcd => {
                        vtr_convert::vcd::convert_vcd(&input, &mut writer, &display.reporter)
                    }
                    Format::Fst => {
                        vtr_convert::fst::convert_fst(&input, &mut writer, &display.reporter)
                    }
                }
                .with_context(|| format!("converting {}", input.display()))?;
                display
                    .reporter
                    .begin(vtr_progress::Phase::Finalizing, None);
                writer.close()?;
                Ok(())
            })?;
            display.complete(&output);
            if !matches!(progress, progress::Style::Json) {
                println!("wrote {}", output.display());
            }
        }
        Command::ToVcd { input, output } => {
            let reader = Reader::open(input)?;
            let changes = output_file(&output, |path| {
                vtr_cli::vcdout::write_vcd(
                    &reader,
                    std::io::BufWriter::new(std::fs::File::create(path)?),
                )
            })?;
            println!("wrote {} changes to {}", changes, output.display());
        }
        Command::Recover { input, output } => {
            let dropped = output_file(&output, |path| Ok(vtr::recover(&input, path)?))?;
            println!(
                "wrote {}; dropped {} bytes",
                output.display(),
                dropped.unwrap_or(0)
            );
            if dropped.is_some_and(|n| n > 0) {
                return Ok(3);
            }
        }
        Command::Index {
            file,
            check,
            threads,
            memory,
        } => {
            let reader = Reader::open(&file)?;
            let index = activity_index(&file, &reader, (!check).then_some((threads, memory)))?;
            let Some(index) = index else {
                println!("no valid activity index; run vtr index <file>");
                return Ok(1);
            };
            println!("valid activity index: {} blocks", index.blocks().len());
        }
        Command::Active {
            file,
            window,
            build,
        } => {
            let (from, to) = window.checked()?;
            let reader = Reader::open(&file)?;
            let index = activity_index(&file, &reader, build.then_some((0, 256)))?
                .context("no valid activity index; run vtr index <file> or pass --build")?;
            let classification = index.classify(from, to);
            let mut active: std::collections::BTreeSet<_> =
                classification.active.into_iter().collect();
            active.extend(vtr::activity::resolve(
                &reader,
                &classification.undecided,
                from,
                to,
            )?);
            for id in reader.hierarchy().ids() {
                if let Some(signal) = reader.hierarchy().signal_of(id)
                    && active.remove(&signal)
                {
                    println!("{}", reader.full_path(id, "."));
                }
            }
        }
    }
    Ok(0)
}
fn main() -> std::process::ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code.into(),
        Err(error) => {
            eprintln!("error: {error:#}");
            2.into()
        }
    }
}
