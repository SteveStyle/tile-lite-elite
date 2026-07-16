use dioxus::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum AuthMode {
    Login,
    Register,
}

/// Shared by the submit button's onclick and the Enter-key handler on each
/// input — a plain function taking every signal it needs explicitly rather
/// than a shared closure, since a closure capturing non-Copy state (the
/// server URL) can't be reused across several `move` handlers in the same
/// render without fighting the borrow checker.
#[allow(clippy::too_many_arguments)]
fn submit_login_or_register(
    server_url: String,
    mode: AuthMode,
    display_name: Signal<String>,
    email: Signal<String>,
    password: Signal<String>,
    remember_me: Signal<bool>,
    stay_logged_in: Signal<bool>,
    mut is_submitting: Signal<bool>,
    mut error_message: Signal<Option<String>>,
    on_authenticated: EventHandler<(api::PlayerSessionDto, bool, bool)>,
) {
    let name = display_name().trim().to_string();
    let email_value = email().trim().to_string();
    let password_value = password();
    let remember = remember_me();
    let stay = stay_logged_in();

    if name.is_empty() || password_value.is_empty() {
        error_message.set(Some("Display name and password are required".to_string()));
        return;
    }
    if mode == AuthMode::Register && email_value.is_empty() {
        error_message.set(Some("Email is required to register".to_string()));
        return;
    }

    spawn(async move {
        is_submitting.set(true);
        error_message.set(None);
        let outcome = match mode {
            AuthMode::Login => crate::app::login_player(&server_url, &name, &password_value).await,
            AuthMode::Register => {
                crate::app::register_player(&server_url, &name, &email_value, &password_value).await
            }
        };
        match outcome {
            Ok(session) => on_authenticated.call((session, remember, stay)),
            Err(error) => error_message.set(Some(error)),
        }
        is_submitting.set(false);
    });
}

#[component]
pub fn AuthPanel(
    server_url: String,
    session: Option<api::PlayerSessionDto>,
    on_authenticated: EventHandler<(api::PlayerSessionDto, bool, bool)>,
    on_logout: EventHandler<()>,
    on_password_changed: EventHandler<()>,
) -> Element {
    let stored = crate::local_storage::load();
    let has_remembered_name = stored.remembered_name.is_some();
    let remembered_name = stored.remembered_name.unwrap_or_default();

    let mut mode = use_signal(|| AuthMode::Login);
    let mut display_name = use_signal(move || remembered_name);
    let mut email = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut remember_me = use_signal(move || has_remembered_name);
    let mut stay_logged_in = use_signal(|| false);
    let mut error_message = use_signal(|| None::<String>);
    let is_submitting = use_signal(|| false);

    let mut show_change_password = use_signal(|| false);
    let mut current_password_input = use_signal(String::new);
    let mut new_password_input = use_signal(String::new);
    let mut confirm_password_input = use_signal(String::new);
    let mut change_password_error = use_signal(|| None::<String>);
    let mut is_changing_password = use_signal(|| false);

    let mut show_forgot_password = use_signal(|| false);
    let mut forgot_password_email = use_signal(String::new);
    let mut forgot_password_message = use_signal(|| None::<String>);
    let mut is_requesting_reset = use_signal(|| false);

    if let Some(session) = session {
        return rsx! {
            div { class: "auth-widget",
                div { class: "auth-widget-row",
                    span { class: "auth-status", "Logged in as {session.display_name}" }
                    button {
                        class: "toggle-button toggle-button-muted",
                        onclick: move |_| {
                            show_change_password.set(!show_change_password());
                            change_password_error.set(None);
                        },
                        "Change password"
                    }
                    button {
                        class: "toggle-button toggle-button-muted",
                        onclick: move |_| {
                            // Logging out should hand back a clean modal,
                            // not whatever was left over from before — the
                            // password (and, in Register mode, email) must
                            // never carry over. Display name is the one
                            // exception: it survives if "Remember me" is
                            // what's asking it to (see local_storage.rs).
                            mode.set(AuthMode::Login);
                            email.set(String::new());
                            password.set(String::new());
                            stay_logged_in.set(false);
                            if !remember_me() {
                                display_name.set(String::new());
                            }
                            on_logout.call(());
                        },
                        "Log out"
                    }
                }
                if show_change_password() {
                    div { class: "auth-panel",
                        input {
                            class: "auth-input",
                            r#type: "password",
                            placeholder: "Current password",
                            value: "{current_password_input}",
                            oninput: move |event| current_password_input.set(event.value()),
                        }
                        input {
                            class: "auth-input",
                            r#type: "password",
                            placeholder: "New password",
                            value: "{new_password_input}",
                            oninput: move |event| new_password_input.set(event.value()),
                        }
                        input {
                            class: "auth-input",
                            r#type: "password",
                            placeholder: "Confirm new password",
                            value: "{confirm_password_input}",
                            oninput: move |event| confirm_password_input.set(event.value()),
                        }
                        if let Some(error) = change_password_error() {
                            p { class: "error-banner", "{error}" }
                        }
                        div { class: "auth-panel-actions",
                            button {
                                class: "toggle-button toggle-button-muted",
                                disabled: is_changing_password(),
                                onclick: move |_| {
                                    show_change_password.set(false);
                                    change_password_error.set(None);
                                    current_password_input.set(String::new());
                                    new_password_input.set(String::new());
                                    confirm_password_input.set(String::new());
                                },
                                "Cancel"
                            }
                            button {
                                class: "toggle-button",
                                disabled: is_changing_password(),
                                onclick: move |_| {
                                    let server_url = server_url.clone();
                                    let token = session.session_token.clone();
                                    let current = current_password_input();
                                    let new_password_value = new_password_input();
                                    let confirm = confirm_password_input();

                                    if current.is_empty() || new_password_value.is_empty() {
                                        change_password_error.set(Some("Both current and new password are required".to_string()));
                                        return;
                                    }
                                    if new_password_value != confirm {
                                        change_password_error.set(Some("New password and confirmation don't match".to_string()));
                                        return;
                                    }

                                    spawn(async move {
                                        is_changing_password.set(true);
                                        change_password_error.set(None);
                                        match crate::app::change_password(&server_url, &token, &current, &new_password_value).await {
                                            Ok(()) => {
                                                show_change_password.set(false);
                                                current_password_input.set(String::new());
                                                new_password_input.set(String::new());
                                                confirm_password_input.set(String::new());
                                                mode.set(AuthMode::Login);
                                                email.set(String::new());
                                                password.set(String::new());
                                                stay_logged_in.set(false);
                                                if !remember_me() {
                                                    display_name.set(String::new());
                                                }
                                                on_password_changed.call(());
                                            }
                                            Err(error) => change_password_error.set(Some(error)),
                                        }
                                        is_changing_password.set(false);
                                    });
                                },
                                "Update password"
                            }
                        }
                    }
                }
            }
        };
    }

    // Nothing in the app works while signed out (every action needs a
    // player), so this is a blocking modal rather than a dismissable
    // widget — no "Cancel", no collapsed state. It's the first thing you
    // see on open.
    let submit_label = if mode() == AuthMode::Login {
        "Log in"
    } else {
        "Register"
    };
    // Each `move` closure below needs its own owned copy — `server_url` is
    // a plain (non-Copy) String, so one `move` closure capturing it would
    // leave nothing for the rest.
    let server_url_for_display_enter = server_url.clone();
    let server_url_for_email_enter = server_url.clone();
    let server_url_for_password_enter = server_url.clone();
    let server_url_for_button = server_url.clone();
    let server_url_for_forgot_password = server_url.clone();

    rsx! {
        div { class: "modal-backdrop",
        div { class: "auth-panel modal-card",
            h2 { class: "modal-title", "Welcome to Tile Lite Elite" }
            p { class: "modal-copy", "Log in or register to create and play games." }
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
                onkeydown: move |event| {
                    if event.key() == Key::Enter {
                        submit_login_or_register(
                            server_url_for_display_enter.clone(),
                            mode(),
                            display_name,
                            email,
                            password,
                            remember_me,
                            stay_logged_in,
                            is_submitting,
                            error_message,
                            on_authenticated,
                        );
                    }
                },
            }
            if mode() == AuthMode::Register {
                input {
                    class: "auth-input",
                    placeholder: "Email",
                    value: "{email}",
                    oninput: move |event| email.set(event.value()),
                    onkeydown: move |event| {
                        if event.key() == Key::Enter {
                            submit_login_or_register(
                                server_url_for_email_enter.clone(),
                                mode(),
                                display_name,
                                email,
                                password,
                                remember_me,
                                stay_logged_in,
                                is_submitting,
                                error_message,
                                on_authenticated,
                            );
                        }
                    },
                }
            }
            input {
                class: "auth-input",
                r#type: "password",
                placeholder: "Password",
                value: "{password}",
                oninput: move |event| password.set(event.value()),
                onkeydown: move |event| {
                    if event.key() == Key::Enter {
                        submit_login_or_register(
                            server_url_for_password_enter.clone(),
                            mode(),
                            display_name,
                            email,
                            password,
                            remember_me,
                            stay_logged_in,
                            is_submitting,
                            error_message,
                            on_authenticated,
                        );
                    }
                },
            }

            label {
                class: "auth-checkbox-label",
                title: "Pre-fills your display name next time you log in. Doesn't keep you signed in or store your password.",
                input {
                    r#type: "checkbox",
                    checked: remember_me(),
                    oninput: move |event| remember_me.set(event.value() == "true"),
                }
                "Remember me"
            }
            label {
                class: "auth-checkbox-label",
                title: "Keeps you signed in on this device — no need to log in again next time. Leave unchecked on a shared or public computer.",
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

            // Only meaningful once an account exists — Register mode has
            // nothing to "forget" yet.
            if mode() == AuthMode::Login {
                button {
                    class: "toggle-button toggle-button-muted",
                    onclick: move |_| {
                        show_forgot_password.set(!show_forgot_password());
                        forgot_password_message.set(None);
                    },
                    "Forgot password?"
                }
            }
            if show_forgot_password() {
                div { class: "auth-panel",
                    input {
                        class: "auth-input",
                        placeholder: "Email",
                        value: "{forgot_password_email}",
                        oninput: move |event| forgot_password_email.set(event.value()),
                    }
                    if let Some(message) = forgot_password_message() {
                        p { class: "modal-copy", "{message}" }
                    }
                    div { class: "auth-panel-actions",
                        button {
                            class: "toggle-button toggle-button-muted",
                            disabled: is_requesting_reset(),
                            onclick: move |_| {
                                show_forgot_password.set(false);
                                forgot_password_email.set(String::new());
                                forgot_password_message.set(None);
                            },
                            "Cancel"
                        }
                        button {
                            class: "toggle-button",
                            disabled: is_requesting_reset(),
                            onclick: move |_| {
                                let server_url = server_url_for_forgot_password.clone();
                                let email_value = forgot_password_email().trim().to_string();
                                if email_value.is_empty() {
                                    forgot_password_message.set(Some("Enter the email you registered with".to_string()));
                                    return;
                                }
                                spawn(async move {
                                    is_requesting_reset.set(true);
                                    // Same message whether or not the email is
                                    // registered — the server's response
                                    // already doesn't distinguish the two
                                    // cases (see RequestPasswordResetRequest's
                                    // doc comment), so neither should this UI.
                                    let _ = crate::app::request_password_reset(&server_url, &email_value).await;
                                    forgot_password_message.set(Some(
                                        "If that email is registered, a reset link is on its way.".to_string(),
                                    ));
                                    is_requesting_reset.set(false);
                                });
                            },
                            "Send reset link"
                        }
                    }
                }
            }

            div { class: "modal-actions",
                button {
                    class: "toggle-button",
                    disabled: is_submitting(),
                    onclick: move |_| {
                        submit_login_or_register(
                            server_url_for_button.clone(),
                            mode(),
                            display_name,
                            email,
                            password,
                            remember_me,
                            stay_logged_in,
                            is_submitting,
                            error_message,
                            on_authenticated,
                        );
                    },
                    "{submit_label}"
                }
            }
        }
        }
    }
}
