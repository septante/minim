#![forbid(unsafe_code)]

mod player;
/// Types related to tracks
pub mod track;

pub use player::Args;
pub use player::Player;
pub use track::Track;
