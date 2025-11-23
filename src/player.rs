use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::Parser;
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MediaKeyCode};
use image::DynamicImage;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, LineGauge, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState,
    },
};
use ratatui_image::{StatefulImage, picker::Picker, protocol::StatefulProtocol};
use rodio::{OutputStream, OutputStreamBuilder, Sink, Source};
use walkdir::WalkDir;

use crate::track::{CachedField, Track};

const PLACEHOLDER_IMAGE_BYTES: &[u8] = include_bytes!("../placeholder.png");

#[derive(Parser, Debug)]
#[command(version, about)]
/// Command-line arguments for the player
pub struct Args {
    /// Where the player should look for files
    dir: Option<PathBuf>,

    /// Reset library cache
    #[arg(short = 'c', long = "clean")]
    reset_cache: bool,
}

#[non_exhaustive]
/// Theme data for the player UI
struct Theme {
    table_selected_row_bg_focused: Color,
    table_selected_row_fg_focused: Color,
    table_selected_row_bg_unfocused: Color,
    table_selected_row_fg_unfocused: Color,
    progress_bar_unfilled: Color,
    progress_bar_filled: Color,
    sidebar_now_playing_fg: Color,
    sidebar_virtual_queue_fg: Color,
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

#[derive(Debug, Clone)]
enum Message {
    Quit,
    ToggleHelp,
    FocusLibrary,
    FocusSidebar,

    PlayPause,
    NextTrack,
    PrevTrack,
    QueueTrack(Track),
    QueueTrackNext(Track),
    RemoveFromQueue(usize),
    VolumeUp(usize),
    VolumeDown(usize),
    CycleRepeatMode,
    ToggleTrackArt,
    SelectLibraryRow(usize),
    SelectSidebarQueueRow(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunningState {
    Quit,
    Library,
    Sidebar,
}

#[derive(Debug, Clone, Default)]
enum RepeatMode {
    #[default]
    Off,
    Queue,
    Single,
}

#[derive(Debug, Clone)]
struct PlayerSettings {
    repeat_mode: Arc<Mutex<RepeatMode>>,
    show_track_art: bool,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            repeat_mode: Default::default(),
            show_track_art: true,
        }
    }
}

#[derive(Clone)]
struct PlaybackState {
    settings: PlayerSettings,
    sink: Arc<Sink>,
    queue: Arc<Mutex<Vec<Track>>>,
    queue_index: Arc<Mutex<usize>>,
    /// Where to insert [`Track`]s when adding to middle of queue
    insertion_offset: Arc<Mutex<usize>>,
}

struct Model {
    running_state: RunningState,
    show_help: bool,
    tracks: Vec<Track>,
    playback_state: PlaybackState,
    volume_percentage: usize,

    // UI related state
    theme: Theme,
    library_table_state: TableState,
    library_scrollbar_state: ScrollbarState,
    sidebar_table_state: TableState,
    sidebar_scrollbar_state: ScrollbarState,
    image_state: Arc<Mutex<Option<StatefulProtocol>>>,
    last_library_scroll: Instant,
    needs_image_redraw: bool,

    // Resources
    picker: Picker,
    // We need to hold the stream to prevent it from being dropped, even if we don't access it otherwise
    // See https://github.com/RustAudio/rodio/issues/525
    _stream: OutputStream,
}

impl Model {
    /// Handles incoming [`Message`]s
    async fn update(&mut self, message: Message) {
        match message {
            Message::Quit => self.running_state = RunningState::Quit,
            Message::ToggleHelp => self.show_help = !self.show_help,
            Message::SelectLibraryRow(row) => self.select_library_row(row),
            Message::SelectSidebarQueueRow(row) => self.select_sidebar_row(row),
            Message::FocusLibrary => {
                self.running_state = RunningState::Library;
            }
            Message::FocusSidebar => {
                if self.playback_state.queue.lock().unwrap().is_empty() {
                    return;
                }

                if self.sidebar_table_state.selected().is_none() {
                    self.sidebar_table_state
                        .select(Some(*self.playback_state.queue_index.lock().unwrap()));
                }

                self.running_state = RunningState::Sidebar;
            }

            // Playback controls
            Message::VolumeUp(percentage) => {
                self.increment_volume(percentage);
            }
            Message::VolumeDown(percentage) => {
                self.decrement_volume(percentage);
            }
            Message::CycleRepeatMode => {
                self.cycle_repeat_mode();
            }
            Message::PlayPause => {
                let sink = &self.playback_state.sink;
                if sink.is_paused() {
                    sink.play();
                } else {
                    sink.pause();
                }
            }
            Message::PrevTrack => self.previous_track(),
            Message::NextTrack => self.next_track(),
            Message::QueueTrack(track) => {
                self.queue_track(track.clone());
                if self.playback_state.sink.empty() {
                    Self::play_track(&track, &self.playback_state);
                }
            }
            Message::QueueTrackNext(track) => {
                let index = *self.playback_state.queue_index.lock().unwrap();
                let mut offset = self.playback_state.insertion_offset.lock().unwrap();
                *offset += 1;

                let mut queue = self.playback_state.queue.lock().unwrap();

                queue.insert(index + *offset, track.clone());

                self.sidebar_scrollbar_state =
                    self.sidebar_scrollbar_state.content_length(queue.len());

                if self.playback_state.sink.empty() {
                    Self::play_track(&track, &self.playback_state.clone());
                }
            }
            Message::RemoveFromQueue(index) => {
                self.remove_track(index);
            }
            Message::ToggleTrackArt => {
                self.playback_state.settings.show_track_art =
                    !self.playback_state.settings.show_track_art;
            }
        }
    }

    fn cycle_repeat_mode(&mut self) {
        let mut repeat_mode = self.playback_state.settings.repeat_mode.lock().unwrap();
        *repeat_mode = match *repeat_mode {
            RepeatMode::Off => RepeatMode::Queue,
            RepeatMode::Queue => RepeatMode::Single,
            RepeatMode::Single => RepeatMode::Off,
        }
    }

    fn increment_volume(&mut self, percentage: usize) {
        self.volume_percentage += percentage;
        if self.volume_percentage > 100 {
            self.volume_percentage = 100;
        }
        self.playback_state
            .sink
            .set_volume(self.volume_percentage as f32 / 100.0);
    }

    fn decrement_volume(&mut self, percentage: usize) {
        self.volume_percentage = self.volume_percentage.saturating_sub(percentage);
        self.playback_state
            .sink
            .set_volume(self.volume_percentage as f32 / 100.0);
    }

    /// Gets the currently playing [`Track`]
    fn now_playing(&self) -> Option<Track> {
        let queue_guard = self.playback_state.queue.lock().unwrap();
        queue_guard
            .get(*self.playback_state.queue_index.lock().unwrap())
            .cloned()
    }

    fn select_library_row(&mut self, row: usize) {
        self.library_table_state.select(Some(row));
        self.library_scrollbar_state = self.library_scrollbar_state.position(row);

        self.last_library_scroll = Instant::now();
        self.needs_image_redraw = true;
    }

    fn select_sidebar_row(&mut self, row: usize) {
        self.sidebar_table_state.select(Some(row));
        self.sidebar_scrollbar_state = self.sidebar_scrollbar_state.position(row);
    }

    /// Adds a [`Track`] to the queue. Does not add it to the [`Sink`]
    fn queue_track(&mut self, track: Track) {
        let mut queue = self.playback_state.queue.lock().unwrap();
        queue.push(track);
        self.sidebar_scrollbar_state = self.sidebar_scrollbar_state.content_length(queue.len());
    }

    /// Adds a [`Track`] to the [`Sink`] for playback
    fn play_track(track: &Track, playback_state: &PlaybackState) {
        let file = fs::File::open(&track.path)
            .expect("Path should be valid, since we imported these files at startup");

        // Add song to queue. TODO: display error message when attempting to open an unsupported file
        if let Ok(decoder) = rodio::Decoder::try_from(file) {
            *playback_state.insertion_offset.lock().unwrap() = 0;

            let playback_clone = playback_state.clone();
            let on_track_end = move || {
                let mut queue_index = playback_clone.queue_index.lock().unwrap();
                let queue = playback_clone.queue.lock().unwrap();
                match *playback_clone.settings.repeat_mode.lock().unwrap() {
                    RepeatMode::Off => {
                        *queue_index += 1;
                    }
                    RepeatMode::Queue => {
                        *queue_index += 1;
                        if *queue_index >= queue.len() {
                            *queue_index = 0;
                        }
                    }
                    RepeatMode::Single => {
                        // Do nothing because we want to play the same track
                    }
                }
                if let Some(track) = queue.get(*queue_index) {
                    Self::play_track(track, &playback_clone);
                }
            };

            let source = WrappedSource::new(decoder, on_track_end);
            playback_state.sink.append(source);
        }
    }

    /// Skips to the next [`Track`] in the queue. If on the last track, stops playback.
    fn next_track(&mut self) {
        self.playback_state.sink.stop();
        let mut queue_index = self.playback_state.queue_index.lock().unwrap();
        let queue = self.playback_state.queue.lock().unwrap();
        match *self.playback_state.settings.repeat_mode.lock().unwrap() {
            // Note that the behavior here is different from if the track ends normally
            // If we are hitting next we should go to the next track even when repeat is set to single
            RepeatMode::Off | RepeatMode::Single => {
                *queue_index += 1;
            }
            RepeatMode::Queue => {
                *queue_index += 1;
                if *queue_index >= queue.len() {
                    *queue_index = 0;
                }
            }
        }

        if *queue_index > queue.len() {
            *queue_index = queue.len();
        }

        if let Some(track) = queue.get(*queue_index) {
            Self::play_track(track, &self.playback_state);
        }
    }

    /// Plays the previous [`Track`] in the queue. If currently on the first track, restarts playback.
    fn previous_track(&mut self) {
        self.playback_state.sink.stop();

        let mut queue_index = self.playback_state.queue_index.lock().unwrap();
        if *queue_index > 0 {
            *queue_index -= 1;
        }

        let queue = self.playback_state.queue.lock().unwrap();
        if let Some(track) = queue.get(*queue_index) {
            Self::play_track(track, &self.playback_state);
        }
    }

    /// Removes the track at the given index from the queue
    fn remove_track(&mut self, index: usize) {
        let queue_index = *self.playback_state.queue_index.lock().unwrap();
        if index == queue_index {
            self.next_track();
        }

        let mut queue = self.playback_state.queue.lock().unwrap();
        let mut queue_index = self.playback_state.queue_index.lock().unwrap();
        if index < *queue_index {
            *queue_index -= 1;
        }

        queue.remove(index);
        if *queue_index > queue.len() {
            *queue_index = queue.len();
        }
    }
}

/// The player app
pub struct Player {
    args: Args,
    library_root: PathBuf,
    model: Model,
}

impl Player {
    /// Create a new player instance
    pub async fn new(args: Args) -> Result<Self> {
        let stream_handle = OutputStreamBuilder::open_default_stream()?;
        let sink = rodio::Sink::connect_new(stream_handle.mixer());
        let sink = Arc::new(sink);
        let volume_percentage = 50;
        sink.set_volume(volume_percentage as f32 / 100.0);

        let library_root = if let Some(ref dir) = args.dir {
            dir.to_owned()
        } else if let Some(dir) = dirs::audio_dir() {
            dir
        } else {
            std::env::current_dir()?
        };

        let picker = Picker::from_query_stdio()?;
        let playback_state = PlaybackState {
            settings: PlayerSettings::default(),
            queue: Arc::new(Mutex::new(Vec::new())),
            queue_index: Arc::new(Mutex::new(0)),
            insertion_offset: Arc::new(Mutex::new(0)),
            sink,
        };

        let model = Model {
            running_state: RunningState::Library,
            show_help: false,
            tracks: Vec::new(),
            playback_state,
            volume_percentage,

            theme: Theme::default(),
            library_table_state: TableState::default().with_selected(0),
            library_scrollbar_state: ScrollbarState::new(0),
            sidebar_table_state: TableState::default(),
            sidebar_scrollbar_state: ScrollbarState::new(0),
            image_state: Arc::new(Mutex::new(None)),
            last_library_scroll: Instant::now(),
            // Need to draw image for first track, but do it after initial render to reduce startup time
            needs_image_redraw: true,

            picker,
            _stream: stream_handle,
        };

        let mut player = Player {
            args,
            library_root,
            model,
        };

        player.import_tracks();
        player.model.tracks.sort_by(|a, b| {
            Track::compare_by_fields(
                a,
                b,
                &[CachedField::Artist, CachedField::Album, CachedField::Title],
            )
        });

        Ok(player)
    }

    fn get_tracks_from_disk(path: &Path) -> Vec<Track> {
        let files = WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|f| f.file_type().is_file());

        files.flat_map(|f| Track::try_from(f.path())).collect()
    }

    fn import_tracks(&mut self) {
        let mut path = dirs::cache_dir().expect("Missing cache dir?");
        path.push("minim");
        if let Ok(exists) = fs::exists(&path)
            && !exists
        {
            fs::create_dir(&path).unwrap();
        }
        path.push("library.csv");

        self.model.tracks = if !self.args.reset_cache
            && let Ok(tracks) = crate::cache::read_cache(&path)
        {
            tracks
        } else {
            Self::get_tracks_from_disk(&self.library_root)
        };

        crate::cache::write_cache(&path, &self.model.tracks).unwrap();

        self.model.library_scrollbar_state = self
            .model
            .library_scrollbar_state
            .content_length(self.model.tracks.len());
    }

    /// Start the player
    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let tick_rate = Duration::from_millis(100);
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

            if self.model.running_state == RunningState::Quit {
                return Ok(());
            }
        }
    }

    fn on_tick(&mut self) {
        if self.model.playback_state.settings.show_track_art
            && self.model.needs_image_redraw
            && Instant::now() - self.model.last_library_scroll > Duration::from_millis(250)
            && let Some(selection) = self.model.library_table_state.selected()
            && let Some(track) = self.model.tracks.get(selection)
        {
            self.model.needs_image_redraw = false;
            let image_state = self.model.image_state.clone();
            let track = track.clone();
            let picker = self.model.picker.clone();
            tokio::spawn(async move {
                Self::update_track_art(&track, &picker, image_state).await;
            });
        }
    }

    fn placeholder_image() -> DynamicImage {
        image::ImageReader::new(Cursor::new(PLACEHOLDER_IMAGE_BYTES))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
    }

    async fn update_track_art(
        track: &Track,
        picker: &Picker,
        image_state: Arc<Mutex<Option<StatefulProtocol>>>,
    ) {
        let image = match track.track_art_as_dynamic_image().await {
            Ok(image) => image,
            Err(_) => Self::placeholder_image(),
        };

        let image = picker.new_resize_protocol(image);
        let image = Some(image);
        *image_state.lock().unwrap() = image;
    }

    async fn handle_events(&mut self) -> std::io::Result<()> {
        match event::read()? {
            // it's important to check that the event is a key press event as
            // crossterm also emits key release and repeat events on Windows.
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event).await;
            }
            _ => {}
        };
        Ok(())
    }

    async fn handle_key_event(&mut self, key_event: KeyEvent) {
        match (
            &self.model.running_state,
            key_event.modifiers,
            key_event.code,
        ) {
            (_, _, _) if self.model.show_help => {
                self.model.update(Message::ToggleHelp).await;
            }

            (_, KeyModifiers::NONE, KeyCode::Char('q')) => {
                self.model.update(Message::Quit).await;
            }

            (_, _, KeyCode::Char('?')) => {
                self.model.update(Message::ToggleHelp).await;
            }

            // Focus navigation
            (RunningState::Library, KeyModifiers::CONTROL, KeyCode::Char('l'))
            | (RunningState::Library, KeyModifiers::CONTROL, KeyCode::Right) => {
                self.model.update(Message::FocusSidebar).await;
            }
            (RunningState::Sidebar, KeyModifiers::CONTROL, KeyCode::Char('h'))
            | (RunningState::Sidebar, KeyModifiers::CONTROL, KeyCode::Left) => {
                self.model.update(Message::FocusLibrary).await;
            }

            // Library navigation
            (RunningState::Library, KeyModifiers::NONE, KeyCode::Char('j'))
            | (RunningState::Library, KeyModifiers::NONE, KeyCode::Down) => {
                let row = match self.model.library_table_state.selected() {
                    Some(i) => {
                        if i >= self.model.tracks.len() - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };

                self.model.update(Message::SelectLibraryRow(row)).await;
            }
            (RunningState::Library, KeyModifiers::NONE, KeyCode::Char('k'))
            | (RunningState::Library, KeyModifiers::NONE, KeyCode::Up) => {
                let row = match self.model.library_table_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.model.tracks.len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };

                self.model.update(Message::SelectLibraryRow(row)).await;
            }
            (RunningState::Library, _, KeyCode::Home) => {
                self.model.update(Message::SelectLibraryRow(0)).await;
            }
            (RunningState::Library, _, KeyCode::End) => {
                self.model
                    .update(Message::SelectLibraryRow(self.model.tracks.len() - 1))
                    .await;
            }

            // Sidebar queue navigation
            (RunningState::Sidebar, KeyModifiers::NONE, KeyCode::Char('j'))
            | (RunningState::Sidebar, KeyModifiers::NONE, KeyCode::Down) => {
                let row = match self.model.sidebar_table_state.selected() {
                    Some(i) => {
                        if i >= self.model.playback_state.queue.lock().unwrap().len() - 1 {
                            0
                        } else {
                            i + 1
                        }
                    }
                    None => 0,
                };

                self.model.update(Message::SelectSidebarQueueRow(row)).await;
            }
            (RunningState::Sidebar, KeyModifiers::NONE, KeyCode::Char('k'))
            | (RunningState::Sidebar, KeyModifiers::NONE, KeyCode::Up) => {
                let row = match self.model.sidebar_table_state.selected() {
                    Some(i) => {
                        if i == 0 {
                            self.model.playback_state.queue.lock().unwrap().len() - 1
                        } else {
                            i - 1
                        }
                    }
                    None => 0,
                };

                self.model.update(Message::SelectSidebarQueueRow(row)).await;
            }
            (RunningState::Sidebar, _, KeyCode::Home) => {
                self.model.update(Message::SelectSidebarQueueRow(0)).await;
            }
            (RunningState::Sidebar, _, KeyCode::End) => {
                let len = self.model.playback_state.queue.lock().unwrap().len();
                self.model
                    .update(Message::SelectSidebarQueueRow(len - 1))
                    .await;
            }

            (RunningState::Sidebar, KeyModifiers::NONE, KeyCode::Char('d')) => {
                if let Some(index) = self.model.sidebar_table_state.selected() {
                    self.model.update(Message::RemoveFromQueue(index)).await;
                }
            }

            // Volume controls
            (_, _, KeyCode::Media(MediaKeyCode::LowerVolume))
            | (_, KeyModifiers::CONTROL, KeyCode::Char('j'))
            | (_, KeyModifiers::CONTROL, KeyCode::Down) => {
                self.model.update(Message::VolumeDown(5)).await;
            }
            (_, _, KeyCode::Media(MediaKeyCode::RaiseVolume))
            | (_, KeyModifiers::CONTROL, KeyCode::Char('k'))
            | (_, KeyModifiers::CONTROL, KeyCode::Up) => {
                self.model.update(Message::VolumeUp(5)).await;
            }

            // Other settings
            (_, KeyModifiers::NONE, KeyCode::Char('i')) => {
                self.model.update(Message::ToggleTrackArt).await;
            }

            // Playback controls
            (_, _, KeyCode::Media(MediaKeyCode::PlayPause))
            | (_, KeyModifiers::NONE, KeyCode::Char('p')) => {
                self.model.update(Message::PlayPause).await;
            }
            (_, _, KeyCode::Media(MediaKeyCode::TrackPrevious))
            | (_, KeyModifiers::NONE, KeyCode::Char('b')) => {
                self.model.update(Message::PrevTrack).await;
            }
            (_, _, KeyCode::Media(MediaKeyCode::TrackNext))
            | (_, KeyModifiers::NONE, KeyCode::Char('n')) => {
                self.model.update(Message::NextTrack).await;
            }
            (_, KeyModifiers::NONE, KeyCode::Char('r')) => {
                self.model.update(Message::CycleRepeatMode).await;
            }
            (RunningState::Library, mods, KeyCode::Enter) => {
                if let Some(index) = self.model.library_table_state.selected() {
                    let track = self
                        .model
                        .tracks
                        .get(index)
                        .expect("Should be valid index")
                        .clone();

                    match mods {
                        KeyModifiers::ALT => {
                            self.model.update(Message::QueueTrackNext(track)).await;
                        }

                        _ => {
                            self.model.update(Message::QueueTrack(track)).await;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let primary_tab_layout =
            &Layout::horizontal([Constraint::Percentage(80), Constraint::Min(15)]);
        let main_panel_layout =
            &Layout::vertical([Constraint::Percentage(100), Constraint::Min(10)]);

        let panel_splits = main_panel_layout.split(frame.area());
        let primary_tab = primary_tab_layout.split(panel_splits[0]);

        Self::render_library(&mut self.model, frame, primary_tab[0]);
        Self::render_sidebar(&mut self.model, frame, primary_tab[1]);
        Self::render_status_bar(&self.model, frame, panel_splits[1]);

        if self.model.show_help {
            Self::render_help(&self.model, frame);
        }
    }

    fn render_help(_model: &Model, frame: &mut Frame) {
        let area = frame.area();
        let margin = 4;
        let area = area.inner(Margin {
            horizontal: margin * 2,
            vertical: margin,
        });

        let binds = [
            ("Help", "?"),
            ("Quit", "q"),
            ("Scroll Up", "k"),
            ("Scroll Down", "j"),
            ("Add to Queue", "Enter"),
            ("Queue Next", "A-Enter"),
            ("Play/Pause", "p"),
            ("Next Track", "n"),
            ("Previous Track", "b"),
            ("Switch Focus Left", "C-h"),
            ("Switch Focus Right", "C-l"),
            ("Remove from Queue", "d"),
            ("Volume Up", "C-k"),
            ("Volume Down", "C-j"),
            ("Change Repeat Mode", "r"),
            ("Toggle Track Art", "i"),
        ];

        let mut lines: Vec<Line> = binds
            .iter()
            .map(|(action, bind)| {
                let bind = format!("<{bind}>");
                let bind = format!("{bind: >20}");

                let texts = vec![Span::raw(format!("{action: <20}")), Span::raw(bind).bold()];

                Line::from(texts).centered()
            })
            .collect();
        lines.push(Line::raw(""));
        lines.push(Line::raw("Press any button to close this menu").centered());

        let text = Text::from(lines);

        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded);
        let widget = Paragraph::new(text).block(block);

        frame.render_widget(Clear, area);
        frame.render_widget(widget, area);
    }

    fn render_status_bar(model: &Model, frame: &mut Frame, area: Rect) {
        let layout = Layout::vertical([Constraint::Max(1), Constraint::Max(1), Constraint::Max(1)]);
        let bars = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Percentage(80),
            Constraint::Min(1),
            Constraint::Percentage(20),
            Constraint::Min(1),
        ]);

        let status_bar_layout = layout.split(area);
        let gauge_layout = bars.split(status_bar_layout[1]);

        let track = model.now_playing();
        let (label, ratio) = match track {
            Some(track) => {
                let time = model.playback_state.sink.get_pos();
                let duration = track.duration;
                let ratio = time.as_secs() as f64 / duration as f64;

                let time = Track::format_duration(time.as_secs());
                let duration = Track::format_duration(duration);
                (format!("{time}/{duration}"), ratio)
            }
            None => ("0:00/0:00".to_string(), 0.0),
        };

        let spacer = Line::raw(" ");

        let progress_bar = LineGauge::default()
            .filled_style(Style::default().fg(model.theme.progress_bar_filled))
            .unfilled_style(Style::default().fg(model.theme.progress_bar_unfilled))
            .ratio(ratio)
            .label(label);

        let volume_gauge = LineGauge::default()
            .filled_style(Style::default().fg(model.theme.progress_bar_filled))
            .unfilled_style(Style::default().fg(model.theme.progress_bar_unfilled))
            .ratio(model.volume_percentage as f64 / 100.0)
            .label(format!("{}%", model.volume_percentage));

        frame.render_widget(&spacer, gauge_layout[0]);
        frame.render_widget(&progress_bar, gauge_layout[1]);
        frame.render_widget(&spacer, gauge_layout[2]);
        frame.render_widget(&volume_gauge, gauge_layout[3]);
        frame.render_widget(&spacer, gauge_layout[4]);

        let instructions = Line::from("For help, press ?").centered();

        frame.render_widget(instructions, status_bar_layout[2]);
    }

    fn render_library(model: &mut Model, frame: &mut Frame, area: Rect) {
        let selected_row_style = if model.running_state == RunningState::Library {
            Style::default()
                .bg(model.theme.table_selected_row_bg_focused)
                .fg(model.theme.table_selected_row_fg_focused)
        } else {
            Style::default()
                .bg(model.theme.table_selected_row_bg_unfocused)
                .fg(model.theme.table_selected_row_fg_unfocused)
        };

        let header = ["Title", "Artist", "Duration"]
            .into_iter()
            .map(ratatui::widgets::Cell::from)
            .collect::<Row>()
            .bottom_margin(1);

        let rows = model.tracks.iter().map(|track| {
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
        let block = Block::new().borders(Borders::all());

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);

        frame.render_stateful_widget(table.block(block), area, &mut model.library_table_state);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut model.library_scrollbar_state,
        );
    }

    fn render_sidebar(model: &mut Model, frame: &mut Frame, area: Rect) {
        if model.playback_state.settings.show_track_art {
            let layout =
                &Layout::vertical([Constraint::Percentage(100), Constraint::Min(area.width / 2)]);

            let layout = layout.split(area);

            Self::render_queue(model, frame, layout[0]);
            Self::render_track_art(model, frame, layout[1]);
        } else {
            Self::render_queue(model, frame, area);
        }
    }

    fn render_track_art(model: &mut Model, frame: &mut Frame, area: Rect) {
        let image_widget = StatefulImage::default();
        let mut image_state = model.image_state.lock().unwrap();
        if image_state.is_none() {
            let image = Self::placeholder_image();
            let image = model.picker.new_resize_protocol(image);
            *image_state = Some(image);
        }
        if let Some(ref mut image) = *image_state {
            frame.render_stateful_widget(image_widget, area, image);
        }
    }

    fn render_queue(model: &mut Model, frame: &mut Frame, area: Rect) {
        let widths = [
            Constraint::Min(3),
            Constraint::Percentage(90),
            Constraint::Min(6),
        ];

        let table = Table::new(
            model
                .playback_state
                .queue
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, track)| {
                    let queue_index = model.playback_state.queue_index.lock().unwrap();
                    let offset = model.playback_state.insertion_offset.lock().unwrap();
                    let currently_playing = index == *queue_index;
                    let in_temp_queue = index > *queue_index && index <= *queue_index + *offset;
                    let display_index = index + 1;
                    let display_index = if currently_playing {
                        format!("{display_index}*")
                    } else {
                        format!("{display_index}")
                    };

                    let mut row = Row::new(vec![
                        Text::from(display_index),
                        Text::from(track.cached_field_string(CachedField::Title)),
                        Text::from(track.cached_field_string(CachedField::Duration)),
                    ]);

                    match model.running_state {
                        RunningState::Sidebar => {
                            if model.sidebar_table_state.selected() == Some(index) {
                                row = row
                                    .bg(model.theme.table_selected_row_bg_focused)
                                    .fg(model.theme.table_selected_row_fg_focused);
                            } else if currently_playing {
                                row = row.fg(model.theme.sidebar_now_playing_fg);
                            } else if in_temp_queue {
                                row = row.fg(model.theme.sidebar_virtual_queue_fg);
                            }
                        }
                        _ => {
                            if currently_playing {
                                row = row.fg(model.theme.sidebar_now_playing_fg);
                            } else if in_temp_queue {
                                row = row.fg(model.theme.sidebar_virtual_queue_fg);
                            }
                        }
                    }

                    row
                }),
            widths,
        );
        let block = Block::new().borders(Borders::all());

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);

        frame.render_stateful_widget(table.block(block), area, &mut model.sidebar_table_state);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut model.sidebar_scrollbar_state,
        );
    }
}

// https://stackoverflow.com/questions/77876116/how-to-i-detect-when-a-sink-moves-to-the-next-source
struct WrappedSource<S, F> {
    source: S,
    on_track_end: F,
}

impl<S, F> WrappedSource<S, F> {
    fn new(source: S, on_track_end: F) -> Self {
        Self {
            source,
            on_track_end,
        }
    }
}

impl<S, F> Iterator for WrappedSource<S, F>
where
    S: Source,
    F: FnMut(),
{
    type Item = S::Item;

    fn next(&mut self) -> Option<Self::Item> {
        match self.source.next() {
            Some(s) => Some(s),
            None => {
                (self.on_track_end)();
                None
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.source.size_hint()
    }
}

impl<S, F> Source for WrappedSource<S, F>
where
    S: Source,
    F: FnMut(),
{
    fn channels(&self) -> u16 {
        self.source.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }

    fn current_span_len(&self) -> Option<usize> {
        self.source.current_span_len()
    }
}
