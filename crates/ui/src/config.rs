//! Desktop server configuration.
//!
//! The web build derives its API origin from the browser (see `app.rs`'s
//! `server_url`), but a native window has no "origin" to derive from, so it
//! picks from a small set of named environments compiled in below. Which
//! server a given run talks to is always overridable at runtime:
//!   --server-url <url>   use this exact URL
//!   --env <name>          use one of the named environments below
//!
//! With neither flag, the default follows `-r`/`--release` (via
//! `cfg!(debug_assertions)`) — debug and `cargo test` builds default to
//! "local", release builds default to "prod". That's the only build-time
//! distinction; testing a release build against "local" (or a debug build
//! against "prod") is a `--env` flag, not a different build.

use std::sync::OnceLock;

pub struct Environment {
    pub name: &'static str,
    pub server_url: &'static str,
}

pub const ENVIRONMENTS: &[Environment] = &[
    Environment {
        name: "local",
        server_url: "http://127.0.0.1:3000",
    },
    Environment {
        name: "prod",
        server_url: "https://129.151.69.246.sslip.io",
    },
];

const DEFAULT_ENV_NAME: &str = if cfg!(debug_assertions) {
    "local"
} else {
    "prod"
};

fn environment_by_name(name: &str) -> Option<&'static Environment> {
    ENVIRONMENTS.iter().find(|e| e.name == name)
}

fn default_server_url() -> String {
    environment_by_name(DEFAULT_ENV_NAME)
        .unwrap_or(&ENVIRONMENTS[0])
        .server_url
        .to_string()
}

fn resolve_from_args(args: &[String]) -> String {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--server-url" => {
                if let Some(url) = iter.next() {
                    return url.clone();
                }
            }
            "--env" => {
                if let Some(name) = iter.next() {
                    match environment_by_name(name) {
                        Some(env) => return env.server_url.to_string(),
                        None => eprintln!(
                            "Unknown --env '{name}'; known environments: {}",
                            ENVIRONMENTS
                                .iter()
                                .map(|e| e.name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    }
                }
            }
            _ => {}
        }
    }
    default_server_url()
}

static SERVER_URL: OnceLock<String> = OnceLock::new();

/// Call once from `main()`, before launching the app, with the process's
/// CLI args (excluding argv[0]).
pub fn init_from_args(args: &[String]) {
    let _ = SERVER_URL.set(resolve_from_args(args));
}

/// The resolved server URL for this run. Falls back to the compiled-in
/// default environment if `init_from_args` was never called.
pub fn server_url() -> String {
    SERVER_URL.get_or_init(default_server_url).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_falls_back_to_default_env() {
        assert_eq!(resolve_from_args(&args(&[])), default_server_url());
    }

    // Pins the actual debug-vs-release default (rather than just comparing
    // against the function under test) so a change to the `cfg!` branch in
    // `DEFAULT_ENV_NAME` shows up as a real failure, not a tautology.
    #[test]
    #[cfg(debug_assertions)]
    fn debug_build_defaults_to_local() {
        assert_eq!(default_server_url(), "http://127.0.0.1:3000");
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn release_build_defaults_to_prod() {
        assert_eq!(default_server_url(), "https://129.151.69.246.sslip.io");
    }

    #[test]
    fn server_url_flag_wins() {
        assert_eq!(
            resolve_from_args(&args(&["--server-url", "http://example:9999"])),
            "http://example:9999"
        );
    }

    #[test]
    fn env_flag_selects_named_environment() {
        assert_eq!(
            resolve_from_args(&args(&["--env", "prod"])),
            "https://129.151.69.246.sslip.io"
        );
    }

    #[test]
    fn unknown_env_falls_back_to_default() {
        assert_eq!(
            resolve_from_args(&args(&["--env", "bogus"])),
            default_server_url()
        );
    }
}
