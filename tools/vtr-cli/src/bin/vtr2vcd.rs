//! `vtr2vcd <file.vtr> <out.vcd>`: writes the signals of a VTR file as VCD
//! (same as `vtr to-vcd`). Prints the number of value changes written.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: vtr2vcd <file.vtr> <out.vcd>");
        std::process::exit(2);
    }
    let t = std::time::Instant::now();
    match vtr_cli::vcdout::vtr_to_vcd(&args[0], &args[1]) {
        Ok(n) => println!("{{\"changes\": {n}, \"wall_s\": {}}}", t.elapsed().as_secs_f64()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}
