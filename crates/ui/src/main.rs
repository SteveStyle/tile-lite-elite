use dioxus::prelude::*;
mod components;

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus_desktop::{Config, WindowBuilder};

        dioxus_desktop::launch_cfg(
            App,
            Config::new().with_window(
                WindowBuilder::new()
                    .with_title("Scrabble Desktop")
                    .with_resizable(true)
                    .with_min_inner_size(dioxus_desktop::tao::dpi::LogicalSize::new(800, 600))
                    .with_inner_size(dioxus_desktop::tao::dpi::LogicalSize::new(1000, 800)),
            ),
        );
    }

    #[cfg(feature = "web")]
    {
        dioxus::launch(App);
    }
}

#[component]
fn App() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Scrabble App" }
            components::scrabble_board::scrabble_board {}
        }
    }
}
