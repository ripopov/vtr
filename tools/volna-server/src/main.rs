use std::path::PathBuf;
use volna_core::session::OpenSpec;

fn main() {
    if let Err(error) = run() {
        eprintln!("volna-server: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        anyhow::bail!("usage: volna-server <trace.vtr|trace.fst>");
    };
    if path == "--help" {
        println!(
            "Usage: volna-server <trace.vtr|trace.fst>\n\nServes one immutable recording over framed stdin/stdout. Diagnostics use stderr."
        );
        return Ok(());
    }
    anyhow::ensure!(args.next().is_none(), "expected exactly one trace path");
    let path = PathBuf::from(path);
    let stamp = |path: &PathBuf| -> anyhow::Result<_> {
        let metadata = std::fs::metadata(path)?;
        anyhow::ensure!(metadata.is_file(), "trace is not a regular file");
        Ok((metadata.len(), metadata.modified()?))
    };
    let initial = stamp(&path)?;
    let mut id = [0; 8];
    loop {
        getrandom::fill(&mut id).map_err(|e| anyhow::anyhow!("session randomness: {e}"))?;
        if id != [0; 8] {
            break;
        }
    }
    volna_core::remote::server::serve(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        u64::from_le_bytes(id),
        || OpenSpec::Path(path.clone()).open(),
        || {
            anyhow::ensure!(
                stamp(&path)? == initial,
                "recording changed; reopen the trace"
            );
            Ok(())
        },
    )
}
