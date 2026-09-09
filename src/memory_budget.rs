//! Memory policies are resolved against one startup snapshot, before OS limits.
use serde::{Deserialize, Serialize};
use std::{io, sync::OnceLock};

pub const MIB: u64 = 1024 * 1024;

/// Numeric JSON values retain compatibility with the old fixed-MiB settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Budget {
    Fixed(u32),
    Percent { percent: u8 },
}
impl Default for Budget {
    fn default() -> Self {
        Self::Fixed(0)
    }
}
impl std::str::FromStr for Budget {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        if let Some(value) = value.strip_suffix('%') {
            let percent = value
                .parse::<u8>()
                .ok()
                .filter(|value| (1..=100).contains(value))
                .ok_or("percentage must be an integer from 1% to 100%")?;
            Ok(Self::Percent { percent })
        } else {
            value
                .parse::<u32>()
                .map(Self::Fixed)
                .map_err(|_| "memory size must be an unsigned MiB integer or percentage".to_owned())
        }
    }
}
impl Budget {
    pub fn resolve(self, snapshot: &Result<Snapshot, String>) -> Result<u64, String> {
        match self {
            Self::Fixed(mib) => Ok(u64::from(mib) * MIB),
            Self::Percent { percent } if (1..=100).contains(&percent) => {
                let snapshot = snapshot.as_ref().map_err(Clone::clone)?;
                let bytes = (u128::from(snapshot.bytes) * u128::from(percent) / 100) as u64;
                if bytes == 0 {
                    return Err("percentage resolves to zero bytes".to_owned());
                }
                Ok(bytes)
            }
            _ => Err("percentage must be an integer from 1% to 100%".to_owned()),
        }
    }
    pub fn resource_mib(self, snapshot: &Result<Snapshot, String>) -> Result<u32, String> {
        let bytes = self.resolve(snapshot)?;
        if bytes != 0 && bytes < MIB {
            return Err("resource percentage resolves to less than 1 MiB".to_owned());
        }
        u32::try_from(bytes / MIB).map_err(|_| "resource memory limit is too large".to_owned())
    }

    pub fn vm_bytes(self, snapshot: &Result<Snapshot, String>) -> Result<u32, String> {
        let bytes = self.resolve(snapshot)?;
        // Glulx has a 32-bit address space, aligned to 256 bytes.
        let bytes = bytes.min(u64::from(!255u32)) & !255;
        if bytes == 0 {
            return Err("game memory limit must be at least 256 bytes".to_owned());
        }
        Ok(bytes as u32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub bytes: u64,
    pub source: &'static str,
}

pub fn startup_snapshot() -> &'static Result<Snapshot, String> {
    static SNAPSHOT: OnceLock<Result<Snapshot, String>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| {
        detect().map_err(|error| format!("Could not detect startup memory: {error}"))
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceBudgets {
    pub undo_mib: Budget,
    pub graphics_cache_mib: Budget,
    pub text_image_cache_mib: Budget,
    pub decoded_image_mib: Budget,
    pub audio_resource_mib: Budget,
    pub song_pcm_mib: Budget,
}
impl Default for ResourceBudgets {
    fn default() -> Self {
        let defaults = crate::memory::ResourceLimits::default();
        Self {
            undo_mib: Budget::Fixed(defaults.undo_mib),
            graphics_cache_mib: Budget::Fixed(defaults.graphics_cache_mib),
            text_image_cache_mib: Budget::Fixed(defaults.text_image_cache_mib),
            decoded_image_mib: Budget::Fixed(defaults.decoded_image_mib),
            audio_resource_mib: Budget::Fixed(defaults.audio_resource_mib),
            song_pcm_mib: Budget::Fixed(defaults.song_pcm_mib),
        }
    }
}
impl ResourceBudgets {
    pub fn resolve(
        self,
        snapshot: &Result<Snapshot, String>,
    ) -> Result<crate::memory::ResourceLimits, String> {
        let mib = |budget: Budget| budget.resource_mib(snapshot);
        Ok(crate::memory::ResourceLimits {
            undo_mib: mib(self.undo_mib)?,
            graphics_cache_mib: mib(self.graphics_cache_mib)?,
            text_image_cache_mib: mib(self.text_image_cache_mib)?,
            decoded_image_mib: mib(self.decoded_image_mib)?,
            audio_resource_mib: mib(self.audio_resource_mib)?,
            song_pcm_mib: mib(self.song_pcm_mib)?,
        }
        .normalized())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryPolicy {
    pub max_memory_mib: Budget,
    pub max_process_memory_mib: Budget,
    pub resource_limits: ResourceBudgets,
}
impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            max_memory_mib: Budget::Fixed(crate::memory::MAX_MEMORY_SIZE / MIB as u32),
            max_process_memory_mib: Budget::Fixed(0),
            resource_limits: Default::default(),
        }
    }
}
impl MemoryPolicy {
    pub fn load() -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        let path = exe
            .parent()
            .ok_or_else(|| io::Error::other("executable has no parent"))?
            .join("glulx-settings.json");
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "linux")]
fn detect() -> io::Result<Snapshot> {
    linux::detect()
}
#[cfg(windows)]
fn detect() -> io::Result<Snapshot> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: the initialized buffer has the size required by this API.
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of_val(&status) as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Snapshot {
            bytes: status.ullAvailPhys,
            source: "ui.memory_source_windows",
        })
    }
}
#[cfg(not(any(target_os = "linux", windows)))]
fn detect() -> io::Result<Snapshot> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "automatic memory detection is supported on Linux and Windows",
    ))
}

#[cfg(any(target_os = "linux", test))]
mod linux;

/// Command-line overrides are session-only and never serialized to settings.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub game: Option<Budget>,
    pub process: Option<Budget>,
    pub resources: std::collections::BTreeMap<String, Budget>,
}
impl Overrides {
    pub fn apply(&self, mut policy: MemoryPolicy) -> MemoryPolicy {
        if let Some(value) = self.game {
            policy.max_memory_mib = value;
        }
        if let Some(value) = self.process {
            policy.max_process_memory_mib = value;
        }
        for (name, value) in &self.resources {
            match name.as_str() {
                "--max-undo-memory" => policy.resource_limits.undo_mib = *value,
                "--max-graphics-cache" => policy.resource_limits.graphics_cache_mib = *value,
                "--max-text-image-cache" => policy.resource_limits.text_image_cache_mib = *value,
                "--max-decoded-image" => policy.resource_limits.decoded_image_mib = *value,
                "--max-audio-resource" => policy.resource_limits.audio_resource_mib = *value,
                "--max-song-pcm" => policy.resource_limits.song_pcm_mib = *value,
                _ => unreachable!("validated memory option"),
            }
        }
        policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percentages_use_snapshot_and_fixed_sizes_work_without_detection() {
        let snapshot = Ok(Snapshot {
            bytes: 8 * 1024 * MIB,
            source: "test",
        });
        assert_eq!(
            "25%".parse::<Budget>().unwrap().resolve(&snapshot).unwrap(),
            2 * 1024 * MIB
        );
        assert_eq!(
            Budget::Fixed(8192)
                .resolve(&Err("unavailable".into()))
                .unwrap(),
            8192 * MIB
        );
        assert!(
            Budget::Percent { percent: 25 }
                .resolve(&Err("unavailable".into()))
                .is_err()
        );
        for value in ["0%", "101%", "-1%", "1.5%", "4294967296", "bogus"] {
            assert!(value.parse::<Budget>().is_err(), "{value}");
        }
        assert_eq!(
            Budget::Percent { percent: 100 }
                .vm_bytes(&snapshot)
                .unwrap(),
            !255u32
        );
        assert!(Budget::Fixed(0).vm_bytes(&snapshot).is_err());
        assert!(Budget::Percent { percent: 0 }.resolve(&snapshot).is_err());
    }
    #[test]
    fn old_json_and_new_percentages_share_settings_and_cli_does_not_mutate_them() {
        let policy: MemoryPolicy = serde_json::from_str(
            r#"{
            "max_memory_mib": 512, "max_process_memory_mib": {"percent": 75},
            "resource_limits": {"undo_mib": 16, "graphics_cache_mib": {"percent": 10}}
        }"#,
        )
        .unwrap();
        let original = serde_json::to_string(&policy).unwrap();
        let overrides = Overrides {
            game: Some(Budget::Percent { percent: 25 }),
            process: Some(Budget::Fixed(0)),
            resources: [("--max-undo-memory".into(), Budget::Fixed(8192))].into(),
        };
        let effective = overrides.apply(policy);
        let snapshot = Ok(Snapshot {
            bytes: 8 * 1024 * MIB,
            source: "test",
        });
        assert_eq!(
            effective.max_memory_mib.vm_bytes(&snapshot).unwrap(),
            2 * 1024 * MIB as u32
        );
        assert_eq!(
            effective.max_process_memory_mib.resolve(&snapshot).unwrap(),
            0
        );
        let limits = effective.resource_limits.resolve(&snapshot).unwrap();
        assert_eq!(limits.undo_mib, 8192);
        assert_eq!(limits.graphics_cache_mib, 819);
        assert_eq!(serde_json::to_string(&policy).unwrap(), original);
    }
    #[test]
    fn startup_snapshot_is_captured_only_once() {
        assert!(std::ptr::eq(startup_snapshot(), startup_snapshot()));
    }
}
