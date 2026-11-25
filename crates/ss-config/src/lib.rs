//! Application settings and standard locations.
//!
//! Config lives in `~/.config/silhouette-studio/settings.toml`, user data (library, device and
//! material tables) in `~/.local/share/silhouette-studio/`, caches in `~/.cache/silhouette-studio/`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub const APP_DIR_NAME: &str = "silhouette-studio";
const SETTINGS_FILE: &str = "settings.toml";
const MAX_RECENT: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a home directory")]
    NoHome,
    #[error("I/O error on {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("invalid settings file {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("could not serialise settings: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Resolved directories. Construct with [`AppPaths::from_system`] or [`AppPaths::with_root`] (tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl AppPaths {
    pub fn from_system() -> Result<Self, ConfigError> {
        // Empty qualifier/organisation gives plain XDG names: ~/.config/silhouette-studio etc.
        let dirs = ProjectDirs::from("", "", APP_DIR_NAME).ok_or(ConfigError::NoHome)?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// All directories under one root; used by tests and portable mode.
    pub fn with_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join(SETTINGS_FILE)
    }

    pub fn library_dir(&self) -> PathBuf {
        self.data_dir.join("library")
    }

    pub fn devices_file(&self) -> PathBuf {
        self.data_dir.join("devices.toml")
    }

    pub fn materials_file(&self) -> PathBuf {
        self.data_dir.join("materials.toml")
    }

    pub fn ensure(&self) -> Result<(), ConfigError> {
        for dir in [&self.config_dir, &self.data_dir, &self.cache_dir, &self.library_dir()] {
            fs::create_dir_all(dir).map_err(|source| ConfigError::Io { path: dir.clone(), source })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    #[default]
    Mm,
    Inch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub units: Units,
    pub theme: Theme,
    /// Default media name (matches a mat preset such as `12x12`).
    pub default_media: String,
    pub show_grid: bool,
    pub grid_spacing_mm: f64,
    pub snap_to_grid: bool,
    /// Flattening tolerance in mm used for booleans and offsets.
    pub curve_tolerance_mm: f64,
    /// Last selected cutter (USB `vid:pid` or `virtual`).
    pub last_device: Option<String>,
    pub recent_files: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            units: Units::Mm,
            theme: Theme::System,
            default_media: "12x12".into(),
            show_grid: true,
            grid_spacing_mm: 10.0,
            snap_to_grid: false,
            curve_tolerance_mm: 0.02,
            last_device: None,
            recent_files: Vec::new(),
        }
    }
}

impl Settings {
    /// Loads settings; a missing file yields defaults.
    pub fn load(paths: &AppPaths) -> Result<Self, ConfigError> {
        let path = paths.settings_file();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse { path, source }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io { path, source }),
        }
    }

    /// Writes atomically (temp file + rename) so a crash never leaves a truncated file.
    pub fn save(&self, paths: &AppPaths) -> Result<(), ConfigError> {
        paths.ensure()?;
        let path = paths.settings_file();
        let tmp = path.with_extension("toml.tmp");
        let text = toml::to_string_pretty(self)?;
        fs::write(&tmp, text).map_err(|source| ConfigError::Io { path: tmp.clone(), source })?;
        fs::rename(&tmp, &path).map_err(|source| ConfigError::Io { path, source })?;
        Ok(())
    }

    pub fn push_recent(&mut self, file: impl Into<PathBuf>) {
        let file = file.into();
        self.recent_files.retain(|f| f != &file);
        self.recent_files.insert(0, file);
        self.recent_files.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ss-config-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_gives_defaults() {
        let paths = AppPaths::with_root(temp_root("missing"));
        assert_eq!(Settings::load(&paths).unwrap(), Settings::default());
    }

    #[test]
    fn roundtrip_and_recent() {
        let root = temp_root("roundtrip");
        let paths = AppPaths::with_root(&root);
        let mut s = Settings { units: Units::Inch, ..Default::default() };
        for i in 0..20 {
            s.push_recent(format!("/tmp/f{i}.studio3"));
        }
        s.push_recent("/tmp/f5.studio3");
        assert_eq!(s.recent_files.len(), MAX_RECENT);
        assert_eq!(s.recent_files[0], PathBuf::from("/tmp/f5.studio3"));
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths).unwrap(), s);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn system_paths_use_app_name() {
        if let Ok(p) = AppPaths::from_system() {
            assert!(p.config_dir.ends_with(APP_DIR_NAME));
            assert!(p.data_dir.ends_with(APP_DIR_NAME));
        }
    }
}
