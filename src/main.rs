#![forbid(unsafe_code)]

use clap::Parser;
use color_eyre::{Result, eyre::Context};

use minim::{Args, Player};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut player = Player::new(args).await?;

    let mut terminal = ratatui::init();
    let result = player.run(&mut terminal).await;
    ratatui::restore();
    result.wrap_err("")
}
