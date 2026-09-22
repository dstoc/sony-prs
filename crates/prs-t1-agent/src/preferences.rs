//! Durable reader preferences for the native PRS-T1 shell.
//!
//! The preference file contains presentation settings only. Authorization
//! state, synchronized credentials, and the temporary reader library have
//! separate boot-scoped or tmpfs lifetimes.

use crate::orientation::ReaderOrientation;
use prs_markdown::{
    DEFAULT_FONT_SCALE_PERCENT, FONT_SCALE_STEP_PERCENT, MAX_FONT_SCALE_PERCENT,
    MIN_FONT_SCALE_PERCENT,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_PREFERENCES_PATH: &str = "/data/misc/prs-t1/reader-preferences.json";
const CURRENT_FORMAT_VERSION: u32 = 1;

/// The settings that survive a device restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderPreferences {
    pub orientation: ReaderOrientation,
    pub font_scale_percent: u16,
}

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            orientation: ReaderOrientation::Portrait,
            font_scale_percent: DEFAULT_FONT_SCALE_PERCENT,
        }
    }
}

impl ReaderPreferences {
    /// Load preferences from the configured device-local path.
    ///
    /// Missing, malformed, and unsupported files all use the safe defaults.
    /// The caller can still report a write failure when a new preference is
    /// selected; a read failure must not prevent the native shell from booting.
    pub fn load() -> Self {
        Self::load_from(path_from_environment())
    }

    pub fn load_from(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                eprintln!(
                    "standalone-test: reader preferences unavailable at {}: {error}; using defaults",
                    path.display()
                );
                return Self::default();
            }
        };

        match serde_json::from_slice::<StoredReaderPreferences>(&bytes)
            .ok()
            .and_then(StoredReaderPreferences::into_preferences)
        {
            Some(preferences) => preferences,
            None => {
                eprintln!(
                    "standalone-test: reader preferences at {} are invalid; using defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Atomically replace the configured device-local preference file.
    pub fn save(&self) -> io::Result<()> {
        self.save_to(path_from_environment())
    }

    /// Atomically replace a preference file. This is public for deterministic
    /// host tests without touching the device path.
    pub fn save_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let temporary_path = temporary_path(path);
        let mut temporary = TemporaryFile::create(&temporary_path)?;
        let encoded = serde_json::to_vec_pretty(&StoredReaderPreferences::from(*self))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let file = temporary
            .file
            .as_mut()
            .expect("new temporary preference file is open");
        file.write_all(&encoded)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
        drop(temporary.file.take());
        fs::rename(&temporary.path, path)?;
        temporary.committed = true;
        Ok(())
    }
}

pub fn path_from_environment() -> PathBuf {
    env::var_os("PRS_T1_PREFERENCES_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PREFERENCES_PATH))
}

pub fn next_font_scale_percent(value: u16) -> u16 {
    if value >= MAX_FONT_SCALE_PERCENT {
        MIN_FONT_SCALE_PERCENT
    } else {
        value
            .saturating_add(FONT_SCALE_STEP_PERCENT)
            .clamp(MIN_FONT_SCALE_PERCENT, MAX_FONT_SCALE_PERCENT)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredReaderPreferences {
    version: u32,
    orientation: String,
    font_scale_percent: u16,
}

impl From<ReaderPreferences> for StoredReaderPreferences {
    fn from(preferences: ReaderPreferences) -> Self {
        Self {
            version: CURRENT_FORMAT_VERSION,
            orientation: preferences.orientation.label().to_ascii_lowercase(),
            font_scale_percent: preferences.font_scale_percent,
        }
    }
}

impl StoredReaderPreferences {
    fn into_preferences(self) -> Option<ReaderPreferences> {
        if self.version != CURRENT_FORMAT_VERSION || !supported_font_scale(self.font_scale_percent)
        {
            return None;
        }
        let orientation = match self.orientation.as_str() {
            "portrait" => ReaderOrientation::Portrait,
            "landscape" => ReaderOrientation::Landscape,
            _ => return None,
        };
        Some(ReaderPreferences {
            orientation,
            font_scale_percent: self.font_scale_percent,
        })
    }
}

fn supported_font_scale(value: u16) -> bool {
    value >= MIN_FONT_SCALE_PERCENT
        && value <= MAX_FONT_SCALE_PERCENT
        && (value - MIN_FONT_SCALE_PERCENT) % FONT_SCALE_STEP_PERCENT == 0
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("reader-preferences.json");
    path.with_file_name(format!(".{file_name}.tmp.{}", std::process::id()))
}

struct TemporaryFile {
    file: Option<File>,
    path: PathBuf,
    committed: bool,
}

impl TemporaryFile {
    fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: Some(OpenOptions::new().write(true).create_new(true).open(path)?),
            path: path.to_owned(),
            committed: false,
        })
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("prs-t1-preferences-{name}-{unique}"))
    }

    #[test]
    fn missing_preferences_use_portrait_and_standard_font_size() {
        let path = test_path("missing");
        assert_eq!(
            ReaderPreferences::load_from(&path),
            ReaderPreferences::default()
        );
    }

    #[test]
    fn preferences_round_trip_without_credentials_or_content() {
        let path = test_path("round-trip");
        let preferences = ReaderPreferences {
            orientation: ReaderOrientation::Landscape,
            font_scale_percent: 125,
        };

        preferences.save_to(&path).expect("save preferences");
        assert_eq!(ReaderPreferences::load_from(&path), preferences);
        let saved = fs::read_to_string(&path).expect("read preferences");
        assert_eq!(
            saved,
            "{\n  \"version\": 1,\n  \"orientation\": \"landscape\",\n  \"font_scale_percent\": 125\n}\n"
        );
        assert!(!saved.contains("credential"));
        assert!(!saved.contains("token"));
        assert!(!saved.contains("content"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_and_unsupported_preferences_use_defaults() {
        let path = test_path("invalid");
        for contents in [
            "not json",
            r#"{"version":2,"orientation":"landscape","font_scale_percent":125}"#,
            r#"{"version":1,"orientation":"sideways","font_scale_percent":125}"#,
            r#"{"version":1,"orientation":"landscape","font_scale_percent":110}"#,
            r#"{"version":1,"orientation":"landscape","font_scale_percent":125,"token":"secret"}"#,
        ] {
            fs::write(&path, contents).expect("write invalid preferences");
            assert_eq!(
                ReaderPreferences::load_from(&path),
                ReaderPreferences::default()
            );
        }
        let _ = fs::remove_file(path);
    }

    #[test]
    fn font_size_choice_cycles_through_supported_scales() {
        assert_eq!(next_font_scale_percent(75), 100);
        assert_eq!(next_font_scale_percent(100), 125);
        assert_eq!(next_font_scale_percent(125), 150);
        assert_eq!(next_font_scale_percent(150), 75);
    }

    #[test]
    fn atomic_replacement_keeps_a_complete_preference_file() {
        let directory = test_path("directory");
        fs::create_dir_all(&directory).expect("create test directory");
        let path = directory.join("reader-preferences.json");
        ReaderPreferences::default()
            .save_to(&path)
            .expect("save preferences");
        let replacement = ReaderPreferences {
            orientation: ReaderOrientation::Landscape,
            font_scale_percent: 150,
        };
        replacement.save_to(&path).expect("replace preferences");
        assert_eq!(ReaderPreferences::load_from(&path), replacement);
        let _ = fs::remove_dir_all(directory);
    }
}
