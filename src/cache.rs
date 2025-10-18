use std::{fs, path::Path};

use anyhow::Result;

use crate::Track;

pub(crate) fn read_cache(path: &Path) -> Result<Vec<Track>> {
    let file = fs::File::open(path)?;
    let mut reader = csv::Reader::from_reader(file);
    let tracks: Vec<Track> = reader.deserialize().flatten().collect();

    Ok(tracks)
}

pub(crate) fn write_cache(path: &Path, tracks: &[Track]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut writer = csv::Writer::from_writer(file);
    for track in tracks {
        writer.serialize(track)?;
    }

    Ok(())
}
