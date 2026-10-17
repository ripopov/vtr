use std::path::Path;
use std::process::Command;
use vtr_progress::{Phase, Snapshot};

fn convert(input: &Path, output: &Path, style: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vtr"))
        .arg("convert")
        .arg(format!("--progress={style}"))
        .arg(input)
        .arg(output)
        .output()
        .unwrap()
}

#[test]
fn machine_progress_reports_disk_sizes_and_only_completes_after_publication() {
    let directory = tempfile::tempdir().unwrap();
    for name in [
        "values.fst",
        "values-wrapped.fst",
        "pipeline.kanata",
        "waves.vcd",
    ] {
        let input = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../vtr-convert/tests/fixtures")
            .join(name);
        let output = directory.path().join(format!("{name}.vtr"));
        let result = convert(&input, &output, "json");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let records: Vec<Snapshot> = String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(!records.is_empty());
        assert!(records.iter().all(Snapshot::valid));
        assert!(
            records
                .iter()
                .all(|p| p.input_bytes == std::fs::metadata(&input).unwrap().len())
        );
        let complete = records.last().unwrap();
        assert_eq!(complete.phase, Phase::Complete);
        assert_eq!(
            complete.output_bytes,
            std::fs::metadata(&output).unwrap().len()
        );
        assert!(complete.output_bytes > 0);
        assert!(
            result.stderr.is_empty(),
            "human progress must stay out of JSON mode"
        );
    }
    let invalid = directory.path().join("bad.kanata");
    std::fs::write(&invalid, "Kanata\t0004\nI\tbad\n").unwrap();
    let output = directory.path().join("failed.vtr");
    let failed = convert(&invalid, &output, "json");
    assert!(!failed.status.success());
    assert!(!output.exists());
    let records: Vec<Snapshot> = String::from_utf8(failed.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.last().unwrap().phase, Phase::Failed);
    assert!(records.iter().all(|record| record.phase != Phase::Complete));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("line 2"));
}

#[test]
fn human_progress_shows_sizes_and_can_be_suppressed() {
    let directory = tempfile::tempdir().unwrap();
    let input =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../vtr-convert/tests/fixtures/pipeline.kanata");
    let result = convert(&input, &directory.path().join("shown.vtr"), "auto");
    assert!(result.status.success());
    let text = String::from_utf8(result.stderr).unwrap();
    assert!(
        text.contains("100%")
            && text.contains("Input ")
            && text.contains("Output ")
            && text.contains("[====================]"),
        "{text}"
    );
    let result = convert(&input, &directory.path().join("quiet.vtr"), "none");
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    assert!(String::from_utf8(result.stdout).unwrap().contains("wrote"));
}
