//! Builds and runs the C smoke test against the static library.

use std::path::PathBuf;
use std::process::Command;

fn target_dir() -> PathBuf {
    // .../target/<profile>/deps/<test-binary> -> .../target/<profile>
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn c_smoke() {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    if Command::new(&cc).arg("--version").output().is_err() {
        eprintln!("no C compiler ({cc}); skipping");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tdir = target_dir();
    let lib = tdir.join("libvtr.a");
    // `cargo test` does not build staticlib artifacts; build it explicitly for this profile.
    let mut build = Command::new(env!("CARGO"));
    build.args(["build", "-p", "vtr-capi", "--lib"]);
    if tdir.file_name().map(|n| n == "release").unwrap_or(false) {
        build.arg("--release");
    }
    assert!(build.status().unwrap().success(), "cargo build of libvtr failed");
    assert!(lib.exists(), "static library not built: {}", lib.display());
    let out_dir = tdir.join("c_smoke_test");
    std::fs::create_dir_all(&out_dir).unwrap();
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
