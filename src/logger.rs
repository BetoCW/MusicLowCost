//! Logger minimo: escribe a app.log en la carpeta de config (y a stderr en debug).

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

struct FileLogger(Mutex<Option<File>>);

impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with(env!("CARGO_CRATE_NAME")) && m.level() <= log::Level::Info
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{} [{}] {}\n", crate::util::unix_now(), r.level(), r.args());
        #[cfg(debug_assertions)]
        eprint!("{line}");
        if let Some(f) = self.0.lock().unwrap().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

pub fn init(dir: &Path) {
    // Se trunca en cada arranque para que no crezca sin limite.
    let file = File::create(dir.join("app.log")).ok();
    let logger = Box::leak(Box::new(FileLogger(Mutex::new(file))));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
