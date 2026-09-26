//! Clumsies desktop client for Windows and Linux.
//!
//! The design rules this client follows live in DESIGN.md; the engine seam is
//! `engine.rs`; see README.md for how to run it.

mod app;
mod components;
mod engine;
mod screens;
mod shell;
mod sign_in;
mod ui;

use gpui_kit::component::Root;
use gpui_kit::*;

use app::DesktopApp;

fn main() {
    // The section rail, the context bar and the status bar draw icons from the
    // bundled set, so the window needs that asset source before anything else.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets::new(""))
        .run(|cx| {
            gpui_kit::init(cx);
            // The app id groups the window under one desktop entry; the title
            // is what the window list and the compositor show.
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Clumsies".into()),
                    ..Default::default()
                }),
                app_id: Some("ai.clumsies.desktop".into()),
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
