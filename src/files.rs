use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    time::Duration,
};

use color_eyre::{Result, eyre::eyre};
use lofty::{picture::Picture, prelude::*, probe::Probe};
use rodio::{Sample, Source};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum Field {
    Cached { field: CachedField },
    Tag { key: ItemKey },
}

#[non_exhaustive]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum CachedField {
    Title,
    Artist,
    Album,
    Year,
    Genre,
    Duration,
}

impl TryFrom<ItemKey> for CachedField {
    type Error = color_eyre::Report;

    fn try_from(key: ItemKey) -> Result<Self, Self::Error> {
        match key {
            ItemKey::TrackTitle => Ok(Self::Title),
            ItemKey::TrackArtist => Ok(Self::Artist),
            // ItemKey::TrackArtists => todo!(),
            ItemKey::AlbumTitle => Ok(Self::Album),
            // ItemKey::AlbumArtist => todo!(),
            // ItemKey::DiscNumber => todo!(),
            // ItemKey::DiscTotal => todo!(),
            // ItemKey::TrackNumber => todo!(),
            // ItemKey::TrackTotal => todo!(),
            ItemKey::Year => Ok(Self::Year),
            ItemKey::Genre => Ok(Self::Genre),
            _ => Err(eyre!("Unsupported field")),
        }
    }
}

impl TryFrom<CachedField> for ItemKey {
    type Error = color_eyre::Report;

    fn try_from(field: CachedField) -> std::result::Result<Self, Self::Error> {
        match field {
            CachedField::Title => Ok(ItemKey::TrackTitle),
            CachedField::Artist => Ok(ItemKey::TrackArtist),
            CachedField::Album => Ok(ItemKey::AlbumTitle),
            CachedField::Year => Ok(ItemKey::Year),
            CachedField::Genre => Ok(ItemKey::Genre),
            _ => Err(eyre!("Unsupported field")),
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub(crate) struct Track {
    pub(crate) path: PathBuf,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration: u64,
}

impl Track {
    fn tag_to_string(tag: Option<Cow<str>>) -> Option<String> {
        tag.as_deref().map(|x| x.to_owned())
    }

    pub(crate) fn cached_field_string(&self, field: CachedField) -> String {
        match field {
            CachedField::Title => {
                if let Some(title) = self.title.clone() {
                    title
                } else {
                    self.path
                        .file_name()
                        .expect("Path should be valid, since we imported these files at startup")
                        .to_string_lossy()
                        .into_owned()
                }
            }
            CachedField::Artist => self.artist.clone().unwrap_or_default(),
            CachedField::Duration => {
                let secs = self.duration;
                let mins = secs / 60;
                let secs = secs % 60;
                format!("{mins}:{:0>2}", secs)
            }
            _ => {
                if let Ok(key) = field.try_into() {
                    if let Ok(s) = self.tag_string_from_track(key) {
                        s
                    } else {
                        "".to_owned()
                    }
                } else {
                    "".to_owned()
                }
            }
        }
    }

    pub(crate) fn tag_string_from_track(&self, key: ItemKey) -> Result<String> {
        let tagged_file = Probe::open(&self.path)?.read()?;

        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
            .ok_or(eyre!("Couldn't"))?;

        Ok(tag
            .get_string(&key)
            .ok_or(eyre!("Couldn't find tag"))?
            .to_owned())
    }

    pub(crate) fn pictures(&self) -> Result<Vec<Picture>> {
        let tagged_file = Probe::open(&self.path)?.read()?;

        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
            .ok_or(eyre!("Couldn't find tag"))?;

        Ok(tag.pictures().to_vec())
    }
}

impl PartialEq for Track {
    fn eq(&self, other: &Self) -> bool {
        self.path.eq(&other.path)
    }
}

impl Eq for Track {}

impl std::hash::Hash for Track {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state)
    }
}

// Can't add generic implementation for AsRef<Path> :(
// https://github.com/rust-lang/rust/issues/50133
impl TryFrom<&Path> for Track {
    type Error = color_eyre::Report;

    fn try_from(path: &Path) -> Result<Self, Self::Error> {
        Self::try_from(path.to_path_buf())
    }
}

impl TryFrom<PathBuf> for Track {
    type Error = color_eyre::Report;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let tagged_file = Probe::open(&path)?.read()?;

        // Try to get primary tag, then try to find the first tag, otherwise
        // generate an empty tag if none exist
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
            .ok_or(eyre!("Couldn't find tags from file"))?;

        let properties = tagged_file.properties();

        Ok({
            Track {
                path,
                title: Self::tag_to_string(tag.title()),
                artist: Self::tag_to_string(tag.artist()),
                album: Self::tag_to_string(tag.album()),
                duration: properties.duration().as_secs(),
            }
        })
    }
}

// https://stackoverflow.com/questions/77876116/how-to-i-detect-when-a-sink-moves-to-the-next-source
pub(crate) struct WrappedSource<S, F> {
    source: S,
    on_track_end: F,
}

impl<S, F> WrappedSource<S, F> {
    pub(crate) fn new(source: S, on_track_end: F) -> Self {
        Self {
            source,
            on_track_end,
        }
    }
}

impl<S, F> Iterator for WrappedSource<S, F>
where
    S: Source,
    S::Item: Sample,
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
    S::Item: Sample,
    F: FnMut(),
{
    fn current_frame_len(&self) -> Option<usize> {
        self.source.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.source.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
}
