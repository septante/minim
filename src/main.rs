use cursive::{traits::*, views::Dialog};

use cursive_table_view::{TableView, TableViewItem};

#[non_exhaustive]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum Field {
    Title,
    Artist,
}

#[non_exhaustive]
#[derive(Clone, Debug)]
struct Track {
    title: String,
    artist: String,
}

impl TableViewItem<Field> for Track {
    fn to_column(&self, column: Field) -> String {
        match column {
            Field::Title => self.title.clone(),
            Field::Artist => self.artist.clone(),
        }
    }

    fn cmp(&self, other: &Self, column: Field) -> std::cmp::Ordering
    where
        Self: Sized,
    {
        match column {
            Field::Title => self.title.cmp(&other.title),
            Field::Artist => self.artist.cmp(&other.artist),
        }
    }
}

fn main() {
    let mut siv = cursive::default();

    let mut table = TableView::<Track, Field>::new()
        .column(Field::Title, "Title", |c| c.width_percent(20))
        .column(Field::Artist, "Artist", |c| c.width_percent(20));

    let mut sample_data = Vec::new();
    sample_data.push(Track {
        title: "Song1".to_owned(),
        artist: "Artist1".to_owned(),
    });

    table.set_items(sample_data);

    table.set_on_submit(|siv, row, index| {
        // Play song
        todo!()
    });

    siv.add_fullscreen_layer(
        Dialog::around(table.with_name("tracks").min_size((50, 20))).title("Library"),
    );

    siv.add_global_callback('q', |s| s.quit());
    siv.run();
}
