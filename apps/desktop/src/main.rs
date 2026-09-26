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

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use app::DesktopApp;

fn main() {
    // The section rail, the context bar and the status bar draw icons from the
    // bundled set, so the window needs that asset source before anything else.
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets::new(""))
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
