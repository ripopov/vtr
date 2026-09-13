fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--stdio")) {
        eprintln!("usage: vtr-server --stdio TRACE.vtr");
        std::process::exit(2);
    }
    let Some(path) = args.next() else {
        eprintln!("usage: vtr-server --stdio TRACE.vtr");
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("usage: vtr-server --stdio TRACE.vtr");
        std::process::exit(2);
    }
    if let Err(error) = vtr_server::run_stdio(path.into()) {
        eprintln!("vtr-server: {error}");
        std::process::exit(1);
    }
}
