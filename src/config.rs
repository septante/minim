use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use color_eyre::eyre::{self, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Config {
    pub library_root: PathBuf,
    pub theme: String,
}

impl Config {
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)?;

        Config::from_str(&s)
    }
}

impl FromStr for Config {
    type Err = eyre::Report;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let config: Self = toml::from_str(s)?;
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library_root: dirs::audio_dir().unwrap(),
            theme: "default".to_owned(),
        }
    }
}
