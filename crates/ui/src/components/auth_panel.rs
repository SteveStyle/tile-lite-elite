use dioxus::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum AuthMode {
    Login,
    Register,
}

#[component]
pub fn AuthPanel(
    server_url: String,
    session: Option<api::PlayerSessionDto>,
    on_authenticated: EventHandler<(api::PlayerSessionDto, bool, bool)>,
    on_logout: EventHandler<()>,
) -> Element {
    let stored = crate::local_storage::load();
    let has_remembered_name = stored.remembered_name.is_some();
    let remembered_name = stored.remembered_name.unwrap_or_default();

    let mut is_open = use_signal(|| false);
    let mut mode = use_signal(|| AuthMode::Login);
    let mut display_name = use_signal(move || remembered_name);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut remember_me = use_signal(move || has_remembered_name);
    let mut stay_logged_in = use_signal(|| false);
    let mut error_message = use_signal(|| None::<String>);
    let mut is_submitting = use_signal(|| false);

    if let Some(session) = session {
        return rsx! {
            div { class: "auth-widget",
                span { class: "auth-status", "Logged in as {session.display_name}" }
                button {
                    class: "toggle-button toggle-button-muted",
                    onclick: move |_| on_logout.call(()),
                    "Log out"
                }
            }
        };
    }

    if !is_open() {
        return rsx! {
            div { class: "auth-widget",
                button {
                    class: "toggle-button toggle-button-muted",
                    onclick: move |_| is_open.set(true),
                    "Log in"
                }
            }
        };
    }

    let submit_label = if mode() == AuthMode::Login {
        "Log in"
    } else {
        "Register"
    };

    rsx! {
        div { class: "auth-panel",
            div { class: "auth-panel-tabs",
                button {
                    class: if mode() == AuthMode::Login { "auth-tab auth-tab-active" } else { "auth-tab" },
                    onclick: move |_| {
                        mode.set(AuthMode::Login);
                        error_message.set(None);
                    },
                    "Log in"
                }
                button {
                    class: if mode() == AuthMode::Register { "auth-tab auth-tab-active" } else { "auth-tab" },
                    onclick: move |_| {
                        mode.set(AuthMode::Register);
                        error_message.set(None);
                    },
                    "Register"
                }
            }

            input {
                class: "auth-input",
                placeholder: "Display name",
                value: "{display_name}",
                oninput: move |event| display_name.set(event.value()),
            }
            if mode() == AuthMode::Register {
                input {
                    class: "auth-input",
                    placeholder: "Email",
                    value: "{email}",
                    oninput: move |event| email.set(event.value()),
                }
            }
            input {
                class: "auth-input",
                r#type: "password",
                placeholder: "Password",
                value: "{password}",
                oninput: move |event| password.set(event.value()),
            }

            label { class: "auth-checkbox-label",
                input {
                    r#type: "checkbox",
                    checked: remember_me(),
                    oninput: move |event| remember_me.set(event.value() == "true"),
                }
                "Remember me"
            }
            label { class: "auth-checkbox-label",
                input {
                    r#type: "checkbox",
                    checked: stay_logged_in(),
                    oninput: move |event| stay_logged_in.set(event.value() == "true"),
                }
                "Stay logged in"
            }

            if let Some(error) = error_message() {
                p { class: "error-banner", "{error}" }
            }

            div { class: "auth-panel-actions",
                button {
                    class: "toggle-button toggle-button-muted",
                    disabled: is_submitting(),
                    onclick: move |_| {
                        is_open.set(false);
                        error_message.set(None);
                    },
                    "Cancel"
                }
                button {
                    class: "toggle-button",
                    disabled: is_submitting(),
                    onclick: move |_| {
                        let server_url = server_url.clone();
                        let name = display_name().trim().to_string();
                        let email_value = email().trim().to_string();
                        let password_value = password();
                        let remember = remember_me();
                        let stay = stay_logged_in();
                        let mode_value = mode();

                        if name.is_empty() || password_value.is_empty() {
                            error_message.set(Some("Display name and password are required".to_string()));
                            return;
                        }
                        if mode_value == AuthMode::Register && email_value.is_empty() {
                            error_message.set(Some("Email is required to register".to_string()));
                            return;
                        }

                        spawn(async move {
                            is_submitting.set(true);
                            error_message.set(None);
                            let outcome = match mode_value {
                                AuthMode::Login => {
                                    crate::app::login_player(&server_url, &name, &password_value).await
                                }
                                AuthMode::Register => {
                                    crate::app::register_player(&server_url, &name, &email_value, &password_value)
                                        .await
                                }
                            };
                            match outcome {
                                Ok(session) => {
                                    is_open.set(false);
                                    on_authenticated.call((session, remember, stay));
                                }
                                Err(error) => error_message.set(Some(error)),
                            }
                            is_submitting.set(false);
                        });
                    },
                    "{submit_label}"
                }
            }
        }
    }
}
