use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "config.toml";
pub const DEFAULT_FONT_SIZE: u32 = 14;
pub const FONT_SIZES: std::ops::RangeInclusive<u32> = 8..=40;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// SSH remote, e.g. `git@github.com:me/notes.git`. Empty = local commits only.
    pub remote_url: String,
    pub branch: String,
    pub author_name: String,
    pub author_email: String,
    /// Used in commit messages and conflict file names.
    pub device_name: String,
    /// Periodic sync interval.
    pub sync_interval_secs: u64,
    /// Desktop only: override for the notes folder (applied on restart).
    pub notes_dir: Option<PathBuf>,
    /// Font family for the editor; empty = the platform's monospace font.
    pub editor_font: String,
    /// Editor font size in points.
    pub editor_font_size: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            remote_url: String::new(),
            branch: "main".into(),
            author_name: "diari".into(),
            author_email: "diari@localhost".into(),
            device_name: default_device_name(),
            sync_interval_secs: 300,
            notes_dir: None,
            editor_font: String::new(),
            editor_font_size: DEFAULT_FONT_SIZE,
        }
    }
}

fn default_device_name() -> String {
    use rand_core::RngCore;
    let n = rand_core::OsRng.next_u32() & 0xffff;
    format!("{}-{n:04x}", std::env::consts::OS)
}

impl Config {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE);
        let cfg = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| match toml::from_str(&s) {
                Ok(c) => Some(c),
                Err(e) => {
                    log::warn!("invalid {}: {e}", path.display());
                    None
                }
            });
        match cfg {
            Some(c) => c,
            None => {
                let c = Self::default();
                if let Err(e) = c.save(data_dir) {
                    log::warn!("cannot write config: {e}");
                }
                c
            }
        }
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::notes::atomic_write(&data_dir.join(FILE), s.as_bytes())
    }
}
