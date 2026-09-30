use std::path::{Path, PathBuf};
use std::process::Command;

/// Ask Cargo for the static library artifact instead of guessing its cache layout.
pub fn static_library() -> PathBuf {
    static_library_with(&[])
}

/// The static library built with `features`, in a target directory of its own
/// so that test binaries running in parallel never overwrite each other's library.
#[allow(dead_code)]
pub fn static_library_with(features: &[&str]) -> PathBuf {
    let mut build = Command::new(env!("CARGO"));
    build.current_dir(env!("CARGO_MANIFEST_DIR")).args([
        "build",
        "--locked",
        "-p",
        "vtr-capi",
        "--lib",
        "--message-format=json",
    ]);
    if !features.is_empty() {
        build.arg("--features").arg(features.join(","));
        build.arg("--target-dir").arg(Path::new(env!("CARGO_TARGET_TMPDIR")).join(features.join("-")));
    }
    if !cfg!(debug_assertions) {
        build.arg("--release");
    }
    let output = build.output().expect("run Cargo to build libvtr");
    assert!(
        output.status.success(),
        "cargo build of libvtr failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("Cargo JSON is UTF-8")
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| {
            message["reason"] == "compiler-artifact" && message["target"]["name"] == "vtr"
        })
        .flat_map(|message| {
            message["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(PathBuf::from))
                .collect::<Vec<_>>()
        })
        .find(|path| path.file_name() == Some(Path::new("libvtr.a").as_os_str()))
        .filter(|path| path.is_file())
        .expect("Cargo must emit the built libvtr.a artifact")
}
