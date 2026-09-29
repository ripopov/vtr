//! Builds and runs the C smoke test against the static library.

use std::path::PathBuf;
use std::process::Command;

mod support;

#[test]
fn c_smoke() {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(&cc).arg("--version").output().expect("C compiler is required").status.success());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lib = support::static_library();
    let temporary = tempfile::tempdir().unwrap();
    let out_dir = temporary.path();
    let exe = out_dir.join("c_smoke");
    let st = Command::new(&cc)
        .args(["-std=c99", "-Wall", "-Wextra", "-Werror", "-O1"])
        .arg("-I")
        .arg(root.join("include"))
        .arg(root.join("tests/c_smoke.c"))
        .arg(&lib)
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(st.success(), "C compilation failed");
    let out = Command::new(&exe).arg(out_dir.join("c_smoke.vtr")).output().unwrap();
    println!("{}", String::from_utf8_lossy(&out.stdout));
    eprintln!("{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "C smoke test failed");
}
