use std::{
    fs,
    io::BufReader,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use clap::Parser;
use color_eyre::{
    Result,
    eyre::{Context, eyre},
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Row, Table, TableState},
};
use rodio::{OutputStream, Sink};
use walkdir::WalkDir;

use crate::files::{CachedField, Track, WrappedSource};

#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Args {
    /// Where the player should look for files
    pub dir: Option<String>,

    /// Reset library cache
    #[arg(short = 'c', long = "clean")]
    disable_cache: bool,
}

#[non_exhaustive]
struct Theme {
    table_selected_row_bg: Color,
    table_selected_row_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            table_selected_row_bg: Color::Blue,
            table_selected_row_fg: Color::Black,
        }
    }
}

pub struct Player {
    args: Args,
    library_root: PathBuf,
    tracks: Vec<Track>,
    queue: Vec<Track>,
    queue_index: Arc<Mutex<usize>>,

    // UI related state
    theme: Theme,
    exit: bool,
    table_state: TableState,

    sink: Sink,
    // We need to hold the stream to prevent it from being dropped, even if we don't access it otherwise
    // See https://github.com/RustAudio/rodio/issues/525
    _stream: OutputStream,
}

impl Player {
    pub fn new(args: Args) -> Result<Self> {
        let (stream, handle) =
            rodio::OutputStream::try_default().wrap_err("Error opening rodio output stream")?;
        let sink = rodio::Sink::try_new(&handle).wrap_err("Error creating new sink")?;

        let library_root;
        if let Some(ref dir) = args.dir {
            library_root = PathBuf::from_str(dir).expect("Shouldn't fail");
        } else {
            library_root = dirs::audio_dir().ok_or(eyre!("Couldn't find music folder"))?;
        }

        let tracks = Self::get_tracks_from_disk(&library_root);

        let player = Player {
            args,
            library_root,
            tracks,
            queue: Vec::new(),
            queue_index: Arc::new(Mutex::new(0)),
            theme: Theme::default(),
            exit: false,
            table_state: TableState::default().with_selected(0),
            sink,
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
        let primary_tab_layout =
            &Layout::horizontal([Constraint::Percentage(80), Constraint::Min(15)]);
        let main_panel_layout =
            &Layout::vertical([Constraint::Percentage(100), Constraint::Min(10)]);

        let primary_tab = primary_tab_layout.split(frame.area());
        let main_panel = main_panel_layout.split(primary_tab[0]);

        self.render_table(frame, main_panel[0]);
        self.render_sidebar(frame, primary_tab[1]);
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
            KeyCode::Char('j') | KeyCode::Down => self.next_table_row(),
            KeyCode::Char('k') | KeyCode::Up => self.previous_table_row(),
            KeyCode::Char('p') => {
                let sink = &self.sink;
                if sink.is_paused() {
                    sink.play();
                } else {
                    sink.pause();
                }
            }
            KeyCode::Char('n') => {
                self.sink.skip_one();
                *self.queue_index.lock().unwrap() += 1;
            }
            KeyCode::Enter => {
                let track = self
                    .tracks
                    .get(self.table_state.selected().expect("No selected row?"))
                    .expect("Should be valid index");

                let file = fs::File::open(&track.path)
                    .expect("Path should be valid, since we imported these files at startup");

                // Add song to queue. TODO: display error message when attempting to open an unsupported file
                if let Ok(decoder) = rodio::Decoder::new(BufReader::new(file)) {
                    let queue_index = self.queue_index.clone();
                    let source = WrappedSource::new(decoder, move || {
                        *queue_index.lock().unwrap() += 1;
                    });
                    self.sink.append(source);
                }

                self.queue.push(track.clone());
            }
            _ => {}
        }
    }

    fn next_table_row(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.tracks.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn previous_table_row(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.tracks.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let selected_row_style = Style::default()
            .bg(self.theme.table_selected_row_bg)
            .fg(self.theme.table_selected_row_fg);

        let header = ["Title", "Artist", "Duration"]
            .into_iter()
            .map(ratatui::widgets::Cell::from)
            .collect::<Row>()
            .bottom_margin(1);

        let rows = self.tracks.iter().map(|track| {
            Row::new(vec![
                Text::from(track.cached_field_string(CachedField::Title)),
                Text::from(track.cached_field_string(CachedField::Artist)),
                Text::from(format!(
                    "{} ",
                    track.cached_field_string(CachedField::Duration)
                ))
                .right_aligned(),
            ])
        });

        let widths = [
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Min(9),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(selected_row_style);

        frame.render_stateful_widget(table, area, &mut self.table_state);
    }

    fn render_sidebar(&mut self, frame: &mut Frame, area: Rect) {
        let widths = [
            Constraint::Min(3),
            Constraint::Percentage(90),
            Constraint::Min(6),
        ];
        let table = Table::new(
            self.queue.iter().enumerate().map(|(index, track)| {
                let index = index + 1;
                Row::new(vec![
                    Text::from(format!("{index}")),
                    Text::from(track.cached_field_string(CachedField::Title)),
                    Text::from(track.cached_field_string(CachedField::Duration)),
                ])
            }),
            widths,
        );
        let block = Block::new().borders(Borders::all());

        frame.render_widget(table.block(block), area);
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
