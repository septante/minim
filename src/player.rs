use std::{path::PathBuf, str::FromStr, sync::Arc};

use clap::Parser;
use color_eyre::{
    Result,
    eyre::{Context, eyre},
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Row, Table, TableState, Widget},
};
use rodio::OutputStream;
use walkdir::WalkDir;

use crate::files::{CachedField, Track};

#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Args {
    /// Where the player should look for files
    pub dir: Option<String>,

    /// Reset library cache
    #[arg(short = 'c', long = "clean")]
    disable_cache: bool,
}

pub struct Player {
    args: Args,
    library_root: PathBuf,
    tracks: Vec<Track>,
    exit: bool,
    table_state: TableState,

    // We need to hold the stream to prevent it from being dropped, even if we don't access it otherwise
    // See https://github.com/RustAudio/rodio/issues/525
    _stream: OutputStream,
}

impl Player {
    pub fn new(args: Args) -> Result<Self> {
        let (stream, handle) =
            rodio::OutputStream::try_default().wrap_err("Error opening rodio output stream")?;
        let sink = rodio::Sink::try_new(&handle).wrap_err("Error creating new sink")?;
        let shared_sink = Arc::new(sink);

        let library_root;
        if let Some(ref dir) = args.dir {
            library_root = PathBuf::from_str(dir).expect("Shouldn't fail");
        } else {
            library_root = dirs::audio_dir().ok_or(eyre!("Couldn't find music folder"))?;
        }

        let tracks = Self::get_tracks_from_disk(&library_root);

        let mut player = Player {
            args,
            library_root,
            tracks,
            exit: false,
            table_state: TableState::default(),
            _stream: stream,
        };

        Ok(player)
    }

    fn get_tracks_from_disk(path: &PathBuf) -> Vec<Track> {
        let files = WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|f| f.file_type().is_file());

        files.flat_map(|f| Track::try_from(f.path())).collect()
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let main_panel_layout =
            &Layout::vertical([Constraint::Percentage(100), Constraint::Min(10)]);

        let main_panel = main_panel_layout.split(frame.area());

        self.render_table(frame, main_panel[0]);
    }

    fn handle_events(&mut self) -> std::io::Result<()> {
        match event::read()? {
            // it's important to check that the event is a key press event as
            // crossterm also emits key release and repeat events on Windows.
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit = true,
            _ => {}
        }
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.tracks.iter().map(|track| {
            Row::new(vec![
                track.cached_field_string(CachedField::Title),
                track.cached_field_string(CachedField::Artist),
                track.cached_field_string(CachedField::Duration),
            ])
        });

        let widths = [
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Min(9),
        ];

        let table = Table::new(rows, widths);

        frame.render_stateful_widget(table, area, &mut self.table_state);
    }
}

// impl Widget for &Player {
//     fn render(self, area: Rect, buf: &mut Buffer) {
//         let title = Line::from(" Minim ".bold());
//         let instructions = Line::from(vec![
//             " Play/Pause ".into(),
//             "<p>".blue().bold(),
//             " Skip ".into(),
//             "<n>".blue().bold(),
//             " Quit ".into(),
//             "<q> ".blue().bold(),
//         ]);
//         let block = Block::bordered()
//             .title(title.centered())
//             .title_bottom(instructions.centered())
//             .border_set(border::THICK);

//         let counter_text = Text::from(vec![Line::from(vec![
//             "Library: ".into(),
//             "ABC".to_string().yellow(),
//         ])]);

//         Paragraph::new(counter_text)
//             .centered()
//             .block(block)
//             .render(area, buf);
//     }
// }
