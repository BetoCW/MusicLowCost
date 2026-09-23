//! Puente entre el hilo de trabajo y la interfaz: todo cambio visual se manda al hilo
//! de la UI con `invoke_from_event_loop` (los tipos de Slint no son Send).

use crate::images::Pixels;
use crate::model::{Card, Track, fmt_time};
use crate::{AppState, AppWindow, CardRow, TrackRow};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Home,
    Search,
    Detail,
    Library,
    Queue,
}

impl ListKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "home" => Self::Home,
            "search" => Self::Search,
            "detail" => Self::Detail,
            "library" => Self::Library,
            "queue" => Self::Queue,
            _ => return None,
        })
    }
    pub const ALL: [ListKind; 5] = [Self::Home, Self::Search, Self::Detail, Self::Library, Self::Queue];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardListKind {
    Home,
    Search,
    Detail,
    Library,
}

#[derive(Clone)]
pub struct Ui {
    weak: slint::Weak<AppWindow>,
}

fn tracks_model(s: &AppState, kind: ListKind) -> ModelRc<TrackRow> {
    match kind {
        ListKind::Home => s.get_home_tracks(),
        ListKind::Search => s.get_search_tracks(),
        ListKind::Detail => s.get_detail_tracks(),
        ListKind::Library => s.get_library_tracks(),
        ListKind::Queue => s.get_queue(),
    }
}

fn cards_model(s: &AppState, kind: CardListKind) -> ModelRc<CardRow> {
    match kind {
        CardListKind::Home => s.get_home_cards(),
        CardListKind::Search => s.get_search_cards(),
        CardListKind::Detail => s.get_detail_cards(),
        CardListKind::Library => s.get_library_cards(),
    }
}

fn image(p: Option<Pixels>) -> Image {
    p.map(Image::from_rgba8).unwrap_or_default()
}

impl Ui {
    pub fn new(weak: slint::Weak<AppWindow>) -> Self {
        Self { weak }
    }

    pub fn run(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let weak = self.weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                f(&ui);
            }
        });
    }

    pub fn state(&self, f: impl FnOnce(&AppState) + Send + 'static) {
        self.run(move |ui| f(&ui.global::<AppState>()));
    }

    /// Reemplaza una lista de canciones. `covers[i]` son portadas ya en cache.
    pub fn set_tracks(&self, kind: ListKind, tracks: &[Track], covers: Vec<Option<Pixels>>, current: Option<String>) {
        let rows: Vec<(Track, Option<Pixels>)> = tracks.iter().cloned().zip(covers).collect();
        self.state(move |s| {
            let model: Vec<TrackRow> = rows
                .into_iter()
                .map(|(t, c)| TrackRow {
                    current: current.as_deref() == Some(t.id.as_str()),
                    duration: if t.duration > 0 { fmt_time(t.duration as f64).into() } else { SharedString::new() },
                    album: t.album.unwrap_or_default().into(),
                    artist: t.artists.into(),
                    title: t.title.into(),
                    id: t.id.into(),
                    cover: image(c),
                })
                .collect();
            let model = ModelRc::from(Rc::new(VecModel::from(model)));
            match kind {
                ListKind::Home => s.set_home_tracks(model),
                ListKind::Search => s.set_search_tracks(model),
                ListKind::Detail => s.set_detail_tracks(model),
                ListKind::Library => s.set_library_tracks(model),
                ListKind::Queue => s.set_queue(model),
            }
        });
    }

    pub fn set_cards(&self, kind: CardListKind, cards: &[Card], covers: Vec<Option<Pixels>>) {
        let rows: Vec<(Card, Option<Pixels>)> = cards.iter().cloned().zip(covers).collect();
        self.state(move |s| {
            let model: Vec<CardRow> = rows
                .into_iter()
                .map(|(c, p)| CardRow {
                    kind: c.kind.as_str().into(),
                    kind_label: c.kind.label().into(),
                    id: c.id.into(),
                    title: c.title.into(),
                    subtitle: c.subtitle.into(),
                    cover: image(p),
                })
                .collect();
            let model = ModelRc::from(Rc::new(VecModel::from(model)));
            match kind {
                CardListKind::Home => s.set_home_cards(model),
                CardListKind::Search => s.set_search_cards(model),
                CardListKind::Detail => s.set_detail_cards(model),
                CardListKind::Library => s.set_library_cards(model),
            }
        });
    }

    /// Pone la portada en la fila `index` si sigue siendo la misma cancion.
    pub fn set_track_cover(&self, kind: ListKind, index: usize, id: String, p: Pixels) {
        self.state(move |s| {
            let m = tracks_model(s, kind);
            if let Some(mut row) = m.row_data(index) {
                if row.id == id.as_str() {
                    row.cover = Image::from_rgba8(p);
                    m.set_row_data(index, row);
                }
            }
        });
    }

    pub fn set_card_cover(&self, kind: CardListKind, index: usize, id: String, p: Pixels) {
        self.state(move |s| {
            let m = cards_model(s, kind);
            if let Some(mut row) = m.row_data(index) {
                if row.id == id.as_str() {
                    row.cover = Image::from_rgba8(p);
                    m.set_row_data(index, row);
                }
            }
        });
    }

    /// Marca la cancion actual en todas las listas visibles.
    pub fn mark_current(&self, id: Option<String>) {
        self.state(move |s| {
            for kind in ListKind::ALL {
                let m = tracks_model(s, kind);
                for i in 0..m.row_count() {
                    if let Some(mut row) = m.row_data(i) {
                        let cur = id.as_deref() == Some(row.id.as_str());
                        if row.current != cur {
                            row.current = cur;
                            m.set_row_data(i, row);
                        }
                    }
                }
            }
        });
    }

    pub fn status(&self, msg: impl Into<String>) {
        let msg: String = msg.into();
        self.state(move |s| s.set_status(msg.into()));
    }

    pub fn loading(&self, on: bool) {
        self.state(move |s| s.set_loading(on));
    }
}
