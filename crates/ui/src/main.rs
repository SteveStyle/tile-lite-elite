use dioxus::prelude::*;
mod app;
mod components;
mod local_storage;
mod time_format;
mod views;

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus_desktop::{Config, WindowBuilder};

        dioxus_desktop::launch::launch(
            App,
            vec![],
            vec![Box::new(
                Config::new().with_window(
                    WindowBuilder::new()
                        .with_title("Scrabble Desktop")
                        .with_resizable(true)
                        .with_min_inner_size(dioxus_desktop::tao::dpi::LogicalSize::new(800, 600))
                        // The board is a 15x15 grid sized off the window's
                        // width (aspect-ratio: 1 in CSS), so it needs real
                        // vertical room before the rack panel and its turn
                        // buttons even fit below it — 1150 cut the rack panel
                        // off, 1450 left ~150px of empty space beneath it.
                        .with_inner_size(dioxus_desktop::tao::dpi::LogicalSize::new(1400, 1300)),
                ),
            )],
        );
    }

    #[cfg(all(feature = "web", not(feature = "desktop")))]
    {
        dioxus::launch(App);
    }
}

#[component]
fn App() -> Element {
    rsx! {
        app::RootApp {}
    }
}
