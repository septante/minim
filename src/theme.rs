use ratatui::style::Color;

#[non_exhaustive]
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
