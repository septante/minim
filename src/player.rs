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
    layout::Rect,
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
};
use rodio::OutputStream;

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
    exit: bool,
    args: Args,
    library_root: PathBuf,
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

        let mut player = Player {
            exit: false,
            args,
            library_root,
            _stream: stream,
        };

        Ok(player)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
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
}

impl Widget for &Player {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = Line::from(" Minim ".bold());
        let instructions = Line::from(vec![
            " Play/Pause ".into(),
            "<p>".blue().bold(),
            " Skip ".into(),
            "<n>".blue().bold(),
            " Quit ".into(),
            "<q> ".blue().bold(),
        ]);
        let block = Block::bordered()
            .title(title.centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        let counter_text = Text::from(vec![Line::from(vec![
            "Library: ".into(),
            "ABC".to_string().yellow(),
        ])]);

        Paragraph::new(counter_text)
            .centered()
            .block(block)
            .render(area, buf);
    }
}
