use std::{fs, path::PathBuf};

use color_eyre::eyre::{Result, eyre};

pub(crate) fn create_config_files() -> Result<()> {
    let path = config_dir().ok_or(eyre!(""))?;
    if !path.exists() {
        fs::create_dir(path)?;
    }
    let path = config_file().ok_or(eyre!(""))?;
    if !path.exists() {
        fs::write(path, include_bytes!("../assets/config.toml"))?;
    }

    let mut path = theme_dir().ok_or(eyre!(""))?;
    if !path.exists() {
        fs::create_dir(&path)?;
    }
    path.push("default.toml");
    if !path.exists() {
        fs::write(path, include_bytes!("../assets/theme.toml"))?;
    }

    let path = cache_dir().ok_or(eyre!(""))?;
    if !path.exists() {
        fs::create_dir(path)?;
    }
    Ok(())
}

pub fn cache_dir() -> Option<PathBuf> {
    let mut path = dirs::cache_dir()?;
    path.push("minim");

    Some(path)
}

pub fn config_dir() -> Option<PathBuf> {
    let mut path = dirs::config_dir()?;
    path.push("minim");

    Some(path)
}

pub fn config_file() -> Option<PathBuf> {
    let mut path = self::config_dir()?;
    path.push("config.toml");

    Some(path)
}
pub fn theme_dir() -> Option<PathBuf> {
    let mut path = self::config_dir()?;
    path.push("themes");

    Some(path)
}
