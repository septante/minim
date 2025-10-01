#![forbid(unsafe_code)]

use clap::Parser;
use color_eyre::{Result, eyre::Context};

use minim::{Args, Player};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init();
    let args = Args::parse();
    match Player::new(args).await {
        Ok(mut player) => {
            let result = player.run(&mut terminal).await;
            ratatui::restore();
            result.wrap_err("")
        }
        Err(e) => {
            ratatui::restore();
            Err(e)
        }
    }
}
