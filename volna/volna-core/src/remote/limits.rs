//! The `memory.*` settings a trace is opened with, for local and remote traces.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct Limits {
    #[serde(rename = "memoryMiB")]
    pub memory_mib: u64,
    #[serde(rename = "objectMiB")]
    pub object_mib: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_mib: 512,
            object_mib: 256,
        }
    }
}

impl Limits {
    /// These bound admitted data, not the browser's total process RSS. A host
    /// can still run out of address space below a user-selected large limit.
    pub fn bytes(self) -> anyhow::Result<(u64, u64)> {
        let max = crate::settings::MAX_MEMORY_MIB as u64;
        anyhow::ensure!(
            (1..=max).contains(&self.memory_mib),
            "memory budget must be between 1 and {max} MiB"
        );
        anyhow::ensure!(
            (1..=max).contains(&self.object_mib),
            "object size limit must be between 1 and {max} MiB"
        );
        Ok((self.memory_mib * 1024 * 1024, self.object_mib * 1024 * 1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_preserve_defaults_and_reject_invalid_limits() {
        let defaults: Limits = serde_json::from_str("{}").unwrap();
        assert_eq!(
            defaults.bytes().unwrap(),
            (512 * 1024 * 1024, 256 * 1024 * 1024)
        );
        let custom: Limits = serde_json::from_str(r#"{"memoryMiB":64,"objectMiB":16}"#).unwrap();
        assert_eq!(
            custom.bytes().unwrap(),
            (64 * 1024 * 1024, 16 * 1024 * 1024)
        );
        for invalid in [0, 256 * 1024 + 1, u64::MAX] {
            assert!(
                Limits {
                    memory_mib: invalid,
                    ..defaults
                }
                .bytes()
                .is_err()
            );
            assert!(
                Limits {
                    object_mib: invalid,
                    ..defaults
                }
                .bytes()
                .is_err()
            );
        }
    }
}
