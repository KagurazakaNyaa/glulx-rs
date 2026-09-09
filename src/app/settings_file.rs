//! Portable JSON settings, independent of eframe's desktop session storage.
use super::PlayerSettings;
use std::{fs, io, path::PathBuf};

pub(super) struct SettingsFile {
    pub path: Option<PathBuf>,
    pub error: Option<String>,
    blocked: bool,
    saved: Option<Vec<u8>>,
}

impl SettingsFile {
    pub fn application(legacy: PlayerSettings) -> (Self, PlayerSettings) {
        // Headless unit-test creation contexts must not share a real config.
        #[cfg(test)]
        {
            Self::load(None, legacy)
        }
        #[cfg(not(test))]
        {
            match std::env::current_exe().and_then(|exe| {
                exe.parent()
                    .map(|dir| dir.join("glulx-settings.json"))
                    .ok_or_else(|| io::Error::other("executable has no parent directory"))
            }) {
                Ok(path) => Self::load(Some(path), legacy),
                Err(error) => (
                    Self {
                        path: None,
                        error: Some(error.to_string()),
                        blocked: true,
                        saved: None,
                    },
                    legacy,
                ),
            }
        }
    }

    fn load(path: Option<PathBuf>, legacy: PlayerSettings) -> (Self, PlayerSettings) {
        let mut file = Self {
            path,
            error: None,
            blocked: false,
            saved: None,
        };
        let Some(path) = &file.path else {
            return (file, legacy);
        };
        let read = fs::read(path).and_then(|bytes| {
            let settings = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            file.saved = Some(bytes);
            Ok(settings)
        });
        match read {
            Ok(settings) => (file, settings),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (file, legacy),
            Err(error) => {
                // Never overwrite a malformed or unreadable user-edited file.
                file.error = Some(format!(
                    "Could not read {}: {error}. Correct the file and restart.",
                    path.display()
                ));
                file.blocked = true;
                (file, legacy)
            }
        }
    }

    pub fn save(&mut self, settings: &PlayerSettings) -> bool {
        if self.blocked {
            return false;
        }
        let Some(path) = &self.path else {
            return false;
        };
        let result = (|| -> io::Result<()> {
            let mut bytes = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
            bytes.push(b'\n');
            if self.saved.as_ref() == Some(&bytes) {
                return Ok(());
            }
            let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
            fs::write(&temporary, &bytes)?;
            if let Err(error) = fs::rename(&temporary, path) {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
            self.saved = Some(bytes);
            Ok(())
        })();
        self.error = result
            .err()
            .map(|error| format!("Could not save {}: {error}", path.display()));
        self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::tests::ResourceDirectory;

    #[test]
    fn portable_settings_migrate_override_and_replace_existing_json() {
        let directory = ResourceDirectory::new();
        let path = directory.0.join("glulx-settings.json");
        let legacy = PlayerSettings {
            font_size: 23.0,
            show_log_window: false,
            ..Default::default()
        };
        let (mut file, settings) = SettingsFile::load(Some(path.clone()), legacy);
        assert_eq!(settings.font_size, 23.0);
        assert!(file.save(&settings));
        let (mut file, mut loaded) =
            SettingsFile::load(Some(path.clone()), PlayerSettings::default());
        assert!(!loaded.show_log_window);
        loaded.font_size = 26.0;
        loaded.max_memory_mib = crate::memory_budget::Budget::Fixed(512);
        loaded.max_process_memory_mib = crate::memory_budget::Budget::Percent { percent: 75 };
        loaded.resource_limits.undo_mib = crate::memory_budget::Budget::Fixed(16);
        loaded.language = super::super::LanguagePreference::Chinese;
        assert!(file.save(&loaded));
        let (_, loaded) = SettingsFile::load(Some(path), PlayerSettings::default());
        assert_eq!(loaded.font_size, 26.0);
        assert_eq!(
            loaded.max_memory_mib,
            crate::memory_budget::Budget::Fixed(512)
        );
        assert_eq!(
            loaded.max_process_memory_mib,
            crate::memory_budget::Budget::Percent { percent: 75 }
        );
        assert_eq!(
            loaded.resource_limits.undo_mib,
            crate::memory_budget::Budget::Fixed(16)
        );
        assert_eq!(loaded.language, super::super::LanguagePreference::Chinese);
    }

    #[test]
    fn malformed_json_is_reported_and_never_overwritten() {
        let directory = ResourceDirectory::new();
        let path = directory.0.join("glulx-settings.json");
        fs::write(&path, b"{not valid json").unwrap();
        let (mut file, settings) =
            SettingsFile::load(Some(path.clone()), PlayerSettings::default());
        assert!(file.error.is_some());
        assert!(!file.save(&settings));
        assert_eq!(fs::read(&path).unwrap(), b"{not valid json");
    }
}
