//! In-memory converter host for a disposable browser worker.

use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail, ensure};
use vtr_progress::{Phase, Reporter, Snapshot};

const MAX_BUFFER_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
struct Output {
    bytes: Arc<Mutex<Vec<u8>>>,
    limit: usize,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut output = self.bytes.lock().expect("output lock");
        if bytes.len() > self.limit.saturating_sub(output.len()) {
            return Err(std::io::Error::other(
                "converted VTR exceeds the browser output limit",
            ));
        }
        output
            .try_reserve(bytes.len())
            .map_err(std::io::Error::other)?;
        output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Returns a complete VTR image only after conversion and finalization succeed.
pub fn convert_to_vtr(
    name: &str,
    bytes: Vec<u8>,
    notify: impl Fn(Snapshot) + Send + Sync + 'static,
) -> Result<Vec<u8>> {
    convert_with_limit(name, bytes, notify, MAX_BUFFER_BYTES)
}

fn convert_with_limit(
    name: &str,
    bytes: Vec<u8>,
    notify: impl Fn(Snapshot) + Send + Sync + 'static,
    limit: usize,
) -> Result<Vec<u8>> {
    let output = Output {
        bytes: Arc::default(),
        limit,
    };
    let measured = output.clone();
    let input_bytes = bytes.len() as u64;
    let progress = Reporter::with_observer(move |phase, completed, total| {
        notify(Snapshot {
            version: vtr_progress::VERSION,
            phase,
            completed,
            total,
            input_bytes,
            output_bytes: measured.bytes.lock().expect("output lock").len() as u64,
        });
    });
    progress.begin(Phase::Preparing, None);
    let result = (|| {
        ensure!(
            bytes.len() <= limit,
            "input exceeds the browser input limit"
        );
        let name = name.to_ascii_lowercase();
        let name = name.strip_suffix(".gz").unwrap_or(&name);
        let extension = name.rsplit_once('.').map(|(_, extension)| extension);
        let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
            let counter = progress.begin(Phase::Reading, Some(input_bytes));
            let mut decoded = Vec::new();
            flate2::read::MultiGzDecoder::new(vtr_progress::CountingReader::new(
                Cursor::new(bytes),
                counter,
            ))
            .take(limit as u64 + 1)
            .read_to_end(&mut decoded)
            .context("decompress input")?;
            ensure!(
                decoded.len() <= limit,
                "decompressed input exceeds the browser input limit"
            );
            ensure!(
                !decoded.starts_with(&[0x1f, 0x8b]),
                "nested gzip input is not supported"
            );
            decoded
        } else {
            bytes
        };
        let length = bytes.len() as u64;
        let mut writer = vtr::Writer::from_writer(
            output.clone(),
            vtr::WriterOptions {
                background: false,
                dedup: false,
                compression: vtr::Compression::LZ4,
                ..Default::default()
            },
        )?;
        match extension {
            Some("kanata" | "log") => {
                vtr_convert::kanata::convert_input(
                    Cursor::new(bytes),
                    length,
                    &mut writer,
                    &progress,
                )?;
            }
            Some("fst") => {
                vtr_convert::fst::convert_input(Cursor::new(bytes), &mut writer, &progress)?;
            }
            Some("vcd") => {
                vtr_convert::vcd::convert_input(
                    Cursor::new(bytes),
                    length,
                    &mut writer,
                    &progress,
                )?;
            }
            _ => bail!("Choose an FST, Kanata, LOG, or VCD recording."),
        }
        progress.begin(Phase::Finalizing, None);
        writer.close()?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            progress.begin(Phase::Complete, None);
            Ok(std::mem::take(
                &mut *output.bytes.lock().expect("output lock"),
            ))
        }
        Err(error) => {
            progress.begin(Phase::Failed, None);
            Err(error)
        }
    }
}

#[cfg(target_family = "wasm")]
mod browser {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub fn max_input_bytes() -> usize {
        super::MAX_BUFFER_BYTES
    }

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = globalThis, js_name = volnaConversionProgress)]
        fn progress(json: &str);
    }

    #[wasm_bindgen]
    pub fn convert(name: &str, bytes: Vec<u8>) -> Result<Vec<u8>, JsValue> {
        console_error_panic_hook::set_once();
        super::convert_to_vtr(name, bytes, |snapshot| {
            progress(&serde_json::to_string(&snapshot).expect("progress serialization"));
        })
        .map_err(|error| JsValue::from_str(&format!("{error:#}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_limits_refuse_input_decompression_and_output_without_completing() {
        let input = b"Kanata\t0004\n";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), Default::default());
        encoder.write_all(&[b' '; 1024]).unwrap();
        let compressed = encoder.finish().unwrap();
        for (name, bytes, limit, message) in [
            ("input.kanata", input.to_vec(), 4, "input exceeds"),
            (
                "compressed.kanata.gz",
                compressed,
                128,
                "decompressed input exceeds",
            ),
            ("output.kanata", input.to_vec(), 128, "browser output limit"),
        ] {
            let phases = Arc::new(Mutex::new(Vec::new()));
            let observed = phases.clone();
            let error = convert_with_limit(
                name,
                bytes,
                move |snapshot| {
                    observed.lock().unwrap().push(snapshot.phase);
                },
                limit,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(message), "{error:#}");
            let phases = phases.lock().unwrap();
            assert_eq!(phases.last(), Some(&Phase::Failed));
            assert!(!phases.contains(&Phase::Complete));
        }
    }
}
