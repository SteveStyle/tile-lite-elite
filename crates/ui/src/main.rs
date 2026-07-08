use dioxus::prelude::*;
mod app;
mod components;
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
                        .with_inner_size(dioxus_desktop::tao::dpi::LogicalSize::new(1000, 800)),
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
