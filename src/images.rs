//! Descarga y decodifica portadas pequenas, con un cache acotado en memoria.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use tokio::sync::Semaphore;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// Cuantas imagenes guardar (60 px ~14 KB cada una: el cache ocupa ~3 MB).
const MAX_CACHED: usize = 200;

pub struct Images {
    http: reqwest::Client,
    cache: RefCell<HashMap<String, Pixels>>,
    order: RefCell<VecDeque<String>>,
    sem: Rc<Semaphore>,
}

impl Images {
    pub fn new(http: reqwest::Client) -> Rc<Self> {
        Rc::new(Self {
            http,
            cache: RefCell::new(HashMap::new()),
            order: RefCell::new(VecDeque::new()),
            sem: Rc::new(Semaphore::new(4)),
        })
    }

    pub fn cached(&self, url: &str) -> Option<Pixels> {
        self.cache.borrow().get(url).cloned()
    }

    /// Descarga `url` y la reduce a `size`x`size` como maximo.
    pub async fn get(&self, url: &str, size: u32) -> Option<Pixels> {
        if let Some(p) = self.cached(url) {
            return Some(p);
        }
        let _permit = self.sem.acquire().await.ok()?;
        if let Some(p) = self.cached(url) {
            return Some(p);
        }
        let bytes = self.http.get(url).send().await.ok()?.bytes().await.ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        let img = if img.width() > size || img.height() > size {
            img.thumbnail(size, size)
        } else {
            img
        };
        let rgba = img.to_rgba8();
        let pixels = Pixels::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height());
        self.insert(url.to_string(), pixels.clone());
        Some(pixels)
    }

    fn insert(&self, url: String, p: Pixels) {
        let mut cache = self.cache.borrow_mut();
        let mut order = self.order.borrow_mut();
        if cache.insert(url.clone(), p).is_none() {
            order.push_back(url);
        }
        while order.len() > MAX_CACHED {
            if let Some(old) = order.pop_front() {
                cache.remove(&old);
            }
        }
    }
}
