//! Clumsies desktop client for Windows and Linux.
//!
//! The design rules this client follows live in DESIGN.md; the engine seam is
//! `engine.rs`; see README.md for how to run it.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod assets;
mod bootstrap;
mod components;
mod engine;
mod logging;
mod memory_paths;
mod project_refresh;
mod screens;
mod shell;
mod sign_in;
mod state;
mod timestamps;
mod ui;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use app::DesktopApp;

fn main() {
    // The log starts before anything else, so a failure while the window is
    // being built leaves a reason behind.
    logging::init();
    let startup = bootstrap::ensure_daemon();
    if std::env::args().any(|arg| arg == "--smoke-test") {
        match startup {
            Ok(()) => println!("Packaged daemon IPC is ready"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Err(error) = startup {
        logging::error(&error);
    }
    logging::info(&format!(
        "starting; daemon root {}, log {}",
        std::env::var("CLUMSIES_DAEMON_ROOT").unwrap_or_else(|_| "unset".to_owned()),
        std::env::var("CLUMSIES_DESKTOP_LOG").unwrap_or_else(|_| "beside the daemon".to_owned()),
    ));
    // The window draws its own icons and the sign-in page draws the brand mark,
    // so it needs the asset source before anything else: the client's own two
    // marks in front of the component library's icon set.
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            // The window draws its own title bar with the window controls in
            // it, which is what TitleBar::window_options sets up: the app owns
            // the drag region there, so the compositor must not claim it.
            //
            // The app id groups the window under one desktop entry; the title is
            // what the window list and the compositor show.
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Clumsies".into()),
                    ..TitleBar::window_options().titlebar.unwrap_or_default()
                }),
                app_id: Some("ai.clumsies.desktop".into()),
                app_owns_titlebar_drag: true,
                ..Default::default()
            };
            cx.spawn(async move |cx| {
                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| DesktopApp::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open window");
            })
            .detach();
        });
}
