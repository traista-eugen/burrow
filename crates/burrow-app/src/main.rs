// Release builds are GUI apps: no console window behind the main window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui::{
    App, Application, Bounds, Context, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};

struct Shell {
    status: SharedString,
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap_2()
            .bg(rgb(0x16161a))
            .text_color(rgb(0xe8e8ec))
            .child(div().text_3xl().child("Burrow"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x8a8a96))
                    .child(self.status.clone()),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        // Quit when the last window closes instead of lingering in the background.
        // (Once burrowd exists, agents keep running there regardless.)
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Burrow".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Shell {
                    status: format!(
                        "v{} \u{00b7} protocol v{} \u{00b7} gpui",
                        env!("CARGO_PKG_VERSION"),
                        burrow_protocol::PROTOCOL_VERSION
                    )
                    .into(),
                })
            },
        )
        .expect("failed to open main window");
        cx.activate(true);
    });
}
