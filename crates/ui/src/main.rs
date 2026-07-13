use dioxus::prelude::*;
mod app;
mod components;
#[cfg(not(target_arch = "wasm32"))]
mod config;
mod local_storage;
mod time_format;
mod views;

fn main() {
    #[cfg(feature = "desktop")]
    {
        use dioxus_desktop::{Config, WindowBuilder};

        let args: Vec<String> = std::env::args().skip(1).collect();
        config::init_from_args(&args);

        dioxus_desktop::launch::launch(
            App,
            vec![],
            vec![Box::new(
                Config::new().with_window(
                    WindowBuilder::new()
                        .with_title(format!("Scrabble Desktop v{}", app_version()))
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

/// The `Major.Minor.Patch` release version from Cargo.toml, plus an
/// optional build identifier appended as SemVer build metadata (`+<id>`)
/// when `SCRABBLE_PX_BUILD_ID` is set at compile time — e.g. a git short
/// SHA or CI run number, for telling internal/test builds apart. A
/// production release simply doesn't set that var, so it shows only the
/// three numbers. Distinct from `api::API_VERSION`: this is the build
/// identity, not the wire-contract version checked against the server on
/// connect (see `app.rs`'s `check_api_version`).
#[allow(dead_code)]
fn app_version() -> String {
    format_app_version(env!("CARGO_PKG_VERSION"), option_env!("SCRABBLE_PX_BUILD_ID"))
}

#[allow(dead_code)]
fn format_app_version(pkg_version: &str, build_id: Option<&str>) -> String {
    match build_id {
        Some(id) if !id.is_empty() => format!("{pkg_version}+{id}"),
        _ => pkg_version.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_build_id_is_three_numbers_only() {
        assert_eq!(format_app_version("0.1.0", None), "0.1.0");
    }

    #[test]
    fn empty_build_id_is_treated_as_absent() {
        assert_eq!(format_app_version("0.1.0", Some("")), "0.1.0");
    }

    #[test]
    fn build_id_appends_as_semver_build_metadata() {
        assert_eq!(
            format_app_version("0.1.0", Some("a1c9f02")),
            "0.1.0+a1c9f02"
        );
    }
}
