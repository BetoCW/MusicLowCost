// En release no abrir consola.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod integrations;
mod logger;
mod media_controls;
mod player;
mod shortcuts;
mod state;
mod tray;
mod util;
mod webview_tweaks;

use state::AppState;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Todo el JS que se inyecta en music.youtube.com, en un solo IIFE para no ensuciar
/// el scope global de la pagina. Solo corre en music.youtube.com (no en el login de Google).
const INIT_SCRIPT: &str = concat!(
    "(function(){\nif (location.hostname !== 'music.youtube.com') return;\n",
    include_str!("inject/adblock.js"),
    "\n",
    include_str!("inject/vendor/cpu-tamer-by-animationframe.js"),
    "\n",
    include_str!("inject/vendor/cpu-tamer-by-dom-mutation.js"),
    "\n",
    include_str!("inject/vendor/rm3.js"),
    "\n",
    include_str!("inject/app.js"),
    "\n})();"
);

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            player::show_window(app);
        }))
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| shortcuts::handler(app, shortcut, event))
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::yir_log,
            commands::yir_get_config,
            commands::yir_set_config,
            commands::yir_song_changed,
            commands::yir_play_state,
            commands::yir_fetch_lyrics,
            commands::yir_sponsor_segments,
            commands::yir_lastfm_connect,
            commands::yir_download_current,
        ])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            logger::init(&config_dir);
            let cfg = config::load(&config_dir);
            log::info!("iniciando; config en {}", config_dir.display());

            app.manage(AppState {
                config_dir,
                config: std::sync::RwLock::new(cfg.clone()),
                playback: Default::default(),
                http: reqwest::Client::builder()
                    .user_agent("YoutubeInRustWeb/0.1")
                    .timeout(std::time::Duration::from_secs(15))
                    .build()?,
            });
            app.manage(media_controls::MediaState::default());
            app.manage(integrations::discord::Discord::spawn());
            app.manage(shortcuts::Bindings::default());

            let window = WebviewWindowBuilder::new(
                app,
                player::MAIN,
                WebviewUrl::External("https://music.youtube.com".parse()?),
            )
            .title("YoutubeInRustWeb")
            .inner_size(1200.0, 800.0)
            .min_inner_size(420.0, 320.0)
            .initialization_script(INIT_SCRIPT)
            .additional_browser_args(&webview_tweaks::browser_args(cfg.general.low_memory_flags))
            .background_color(tauri::webview::Color(3, 3, 3, 255))
            .visible(false)
            .build()?;

            if cfg.adblock {
                webview_tweaks::install_adblock(&window);
            }
            if cfg.general.start_minimized {
                webview_tweaks::set_low_memory(&window, true);
            } else {
                window.show()?;
            }

            let handle = app.handle();
            media_controls::init(handle, &window);
            tray::init(handle)?;
            shortcuts::apply(handle, &cfg.shortcuts);
            if cfg.api_server.enabled {
                integrations::api_server::spawn(handle.clone(), cfg.api_server.port);
            }
            integrations::spawn_ticker(handle.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            let app = window.app_handle();
            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    if app.state::<AppState>().config().general.close_to_tray {
                        api.prevent_close();
                        player::hide_window(app);
                    }
                }
                WindowEvent::Resized(_) => {
                    let low = app.state::<AppState>().config().general.low_memory_when_hidden;
                    if let Some(w) = app.get_webview_window(player::MAIN) {
                        if w.is_minimized().unwrap_or(false) {
                            if low {
                                webview_tweaks::set_low_memory(&w, true);
                            }
                        } else if w.is_visible().unwrap_or(false) {
                            webview_tweaks::set_low_memory(&w, false);
                        }
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error al iniciar YoutubeInRustWeb");
}
