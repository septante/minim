use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::Parser;
use color_eyre::{Result, eyre::eyre};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use image::{DynamicImage, ImageBuffer, ImageDecoder, Rgb, codecs::jpeg::JpegDecoder};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, LineGauge, Row, Table, TableState},
};
use ratatui_image::{StatefulImage, picker::Picker, protocol::StatefulProtocol};
use rodio::{OutputStream, OutputStreamBuilder, Sink};
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
    progress_bar_unfilled: Color,
    progress_bar_filled: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            table_selected_row_bg: Color::Blue,
            table_selected_row_fg: Color::Black,
            progress_bar_unfilled: Color::White,
            progress_bar_filled: Color::Blue,
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
    picker: Picker,
    image_state: Arc<Mutex<StatefulProtocol>>,

    sink: Sink,
    // We need to hold the stream to prevent it from being dropped, even if we don't access it otherwise
    // See https://github.com/RustAudio/rodio/issues/525
    _stream: OutputStream,
}

impl Player {
    pub async fn new(args: Args) -> Result<Self> {
        let stream_handle = OutputStreamBuilder::open_default_stream()?;
        let sink = rodio::Sink::connect_new(stream_handle.mixer());

        let library_root;
        if let Some(ref dir) = args.dir {
            library_root = PathBuf::from_str(dir).expect("Shouldn't fail");
        } else {
            library_root = dirs::audio_dir().ok_or(eyre!("Couldn't find music folder"))?;
        }

        let mut tracks = Self::get_tracks_from_disk(&library_root);
        tracks.sort_by(|a, b| {
            Track::compare_by_fields(
                a,
                b,
                vec![CachedField::Artist, CachedField::Album, CachedField::Title],
            )
        });

        let picker = Picker::from_query_stdio()?;

        let dyn_image = if let Some(track) = tracks.first() {
            Self::track_art_as_dynamic_image(track).await
        } else {
            DynamicImage::default()
        };
        let image_state = picker.new_resize_protocol(dyn_image);
        let image_state = Arc::new(Mutex::new(image_state));

        let player = Player {
            args,
            library_root,
            tracks,
            queue: Vec::new(),
            queue_index: Arc::new(Mutex::new(0)),
            theme: Theme::default(),
            exit: false,
            table_state: TableState::default().with_selected(0),
            picker,
            image_state,
            sink,
            _stream: stream_handle,
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

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();

        loop {
            terminal.draw(|frame| self.draw(frame))?;
            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_events().await?;
            }

            if last_tick.elapsed() >= tick_rate {
                self.on_tick();
                last_tick = Instant::now();
            }

            if self.exit {
                return Ok(());
            }
        }
    }

    fn on_tick(&mut self) {
        // Put any update things here
    }

    fn draw(&mut self, frame: &mut Frame) {
        let primary_tab_layout =
            &Layout::horizontal([Constraint::Percentage(80), Constraint::Min(15)]);
        let main_panel_layout =
            &Layout::vertical([Constraint::Percentage(100), Constraint::Min(10)]);

        let panel_splits = main_panel_layout.split(frame.area());
        let primary_tab = primary_tab_layout.split(panel_splits[0]);

        self.render_table(frame, primary_tab[0]);
        self.render_sidebar(frame, primary_tab[1]);
        self.render_status_bar(frame, panel_splits[1]);
    }

    async fn handle_events(&mut self) -> std::io::Result<()> {
        match event::read()? {
            // it's important to check that the event is a key press event as
            // crossterm also emits key release and repeat events on Windows.
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event).await
            }
            _ => {}
        };
        Ok(())
    }

    async fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Char('j') | KeyCode::Down => self.next_table_row().await,
            KeyCode::Char('k') | KeyCode::Up => self.previous_table_row().await,
            KeyCode::Char('p') => {
                let sink = &self.sink;
                if sink.is_paused() {
                    sink.play();
                } else {
                    sink.pause();
                }
            }
            KeyCode::Char('b') => self.previous_track(),
            KeyCode::Char('n') => self.next_track(),
            KeyCode::Enter => {
                let track = self
                    .tracks
                    .get(self.table_state.selected().expect("No selected row?"))
                    .expect("Should be valid index")
                    .clone();

                self.queue_track(&track);

                self.queue.push(track.clone());
            }
            _ => {}
        }
    }

    fn now_playing(&self) -> Option<&Track> {
        self.queue.get(*self.queue_index.lock().unwrap())
    }

    fn queue_track(&mut self, track: &Track) {
        let file = fs::File::open(&track.path)
            .expect("Path should be valid, since we imported these files at startup");

        // Add song to queue. TODO: display error message when attempting to open an unsupported file
        if let Ok(decoder) = rodio::Decoder::try_from(file) {
            let queue_index = self.queue_index.clone();
            let on_track_end = move || {
                *queue_index.lock().unwrap() += 1;
            };
            let source = WrappedSource::new(decoder, on_track_end);
            self.sink.append(source);
        }
    }

    fn next_track(&mut self) {
        self.sink.skip_one();
        let mut index = self.queue_index.lock().unwrap();
        *index += 1;
        if *index > self.queue.len() {
            *index = self.queue.len();
        }
    }

    fn previous_track(&mut self) {
        self.sink.clear();

        {
            let mut index = self.queue_index.lock().unwrap();
            if *index > 0 {
                *index -= 1;
            }
        }

        let iter = self
            .queue
            .clone()
            .into_iter()
            .skip(*self.queue_index.lock().unwrap());
        for track in iter {
            self.queue_track(&track);
        }
        self.sink.play();
    }

    async fn track_art_as_dynamic_image(track: &Track) -> DynamicImage {
        let pictures = track.pictures().unwrap();
        if let Some(picture) = pictures.first() {
            let cursor = Cursor::new(picture.data());
            if let Some(mimetype) = picture.mime_type() {
                match mimetype {
                    lofty::picture::MimeType::Png => todo!(),
                    lofty::picture::MimeType::Jpeg => {
                        if let Ok(decoder) = JpegDecoder::new(cursor) {
                            let width = decoder.dimensions().0;
                            let height = decoder.dimensions().1;

                            let mut buf = vec![0; decoder.total_bytes().try_into().unwrap()];

                            if decoder.read_image(&mut buf).is_ok() {
                                let image: Option<ImageBuffer<Rgb<u8>, Vec<u8>>> =
                                    ImageBuffer::from_raw(width, height, buf);
                                if let Some(image) = image {
                                    return image.into();
                                }
                            }
                        }
                    }
                    lofty::picture::MimeType::Tiff => todo!(),
                    lofty::picture::MimeType::Bmp => todo!(),
                    lofty::picture::MimeType::Gif => todo!(),
                    lofty::picture::MimeType::Unknown(_) => todo!(),
                    _ => todo!(),
                }
            }
        }

        // If it fails for whatever reason, return an empty image
        DynamicImage::default()
    }

    async fn display_track_art(&mut self, track: &Track) {
        let image = Self::track_art_as_dynamic_image(track).await;
        let image_state = self.picker.new_resize_protocol(image);
        *self.image_state.lock().unwrap() = image_state;
    }

    async fn next_table_row(&mut self) {
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

        if let Some(track) = self.tracks.get(self.table_state.selected().unwrap()) {
            self.display_track_art(&track.clone()).await;
        }
    }

    async fn previous_table_row(&mut self) {
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

        if let Some(track) = self.tracks.get(self.table_state.selected().unwrap()) {
            self.display_track_art(&track.clone()).await;
        }
    }

    fn render_status_bar(&mut self, frame: &mut Frame, area: Rect) {
        let layout = Layout::vertical([Constraint::Max(1), Constraint::Max(1)]);
        let layout = layout.split(area);

        let track = self.now_playing();
        let (label, ratio) = match track {
            Some(track) => {
                let time = self.sink.get_pos();
                let duration = track.duration;
                let ratio = time.as_secs() as f64 / duration.as_secs() as f64;

                let time = Track::format_duration(&time);
                let duration = Track::format_duration(&duration);
                (format!("{time}/{duration}"), ratio)
            }
            None => ("0:00/0:00".to_string(), 0.0),
        };

        let progress_bar = LineGauge::default()
            .filled_style(Style::default().fg(self.theme.progress_bar_filled))
            .unfilled_style(Style::default().fg(self.theme.progress_bar_unfilled))
            .ratio(ratio)
            .label(label);
        frame.render_widget(progress_bar, layout[0]);

        let instructions: Vec<Span> = vec![
            " Play/Pause ".into(),
            "<p>".into(),
            " Skip ".into(),
            "<n>".into(),
            " Quit ".into(),
            "<q> ".into(),
        ];
        let instructions = Line::from(instructions).centered();

        frame.render_widget(instructions, layout[1]);
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
        let sidebar_layout =
            &Layout::vertical([Constraint::Percentage(100), Constraint::Min(area.width)]);

        let shapes = sidebar_layout.split(area);

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
        frame.render_widget(table.block(block), shapes[0]);

        let image_widget = StatefulImage::default();
        let image_state = &mut *self.image_state.lock().unwrap();
        frame.render_stateful_widget(image_widget, shapes[1], image_state);
    }
}
