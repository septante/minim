use std::{path::Path, str::FromStr};

use color_eyre::eyre::{self, Result, eyre};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Theme data for the player UI
pub struct Theme {
    pub table_selected_row_bg_focused: Color,
    pub table_selected_row_fg_focused: Color,
    pub table_selected_row_bg_unfocused: Color,
    pub table_selected_row_fg_unfocused: Color,
    pub progress_bar_unfilled: Color,
    pub progress_bar_filled: Color,
    pub sidebar_now_playing_fg: Color,
    pub sidebar_virtual_queue_fg: Color,
}

impl Theme {
    pub fn get_theme_by_name(name: &str) -> Result<Self> {
        let mut path = dirs::config_dir().ok_or(eyre!("Couldn't find config dir"))?;
        path.push("minim");

        path.push(name);

        Self::load_from_file(path)
    }

    fn load_from_file<T>(path: T) -> Result<Self>
    where
        T: AsRef<Path>,
    {
        let s = std::fs::read_to_string(path)?;

        Self::from_str(&s)
    }
}

impl FromStr for Theme {
    type Err = eyre::Report;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let theme: Theme = toml::from_str(s)?;
        Ok(theme)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            table_selected_row_bg_focused: Color::Blue,
            table_selected_row_fg_focused: Color::Black,
            table_selected_row_bg_unfocused: Color::Gray,
            table_selected_row_fg_unfocused: Color::Black,
            progress_bar_unfilled: Color::White,
            progress_bar_filled: Color::Blue,
            sidebar_now_playing_fg: Color::Blue,
            sidebar_virtual_queue_fg: Color::Magenta,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn load_builtin_theme() {
        assert_eq!(
            Theme::default(),
            Theme::from_str(include_str!("../assets/theme.toml")).unwrap()
        )
    }
}
