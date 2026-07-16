use dioxus::prelude::*;

const MAIN_CSS: Asset = asset!("/assets/styling/main.css");

/// Landing page for the link emailed by the "forgot password" flow
/// (`/reset-password?token=...`). Standalone rather than a modal like
/// `AuthPanel`'s change-password form — this is reached by clicking a link
/// from outside the app entirely (an email client), typically with no
/// existing session and no game state to show behind it.
#[component]
pub fn ResetPassword(server_url: String, token: String) -> Element {
    let mut new_password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error_message = use_signal(|| None::<String>);
    let mut is_submitting = use_signal(|| false);
    let mut succeeded = use_signal(|| false);

    if succeeded() {
        return rsx! {
            document::Link { rel: "stylesheet", href: MAIN_CSS }
            div { class: "modal-backdrop",
                div { class: "auth-panel modal-card",
                    h2 { class: "modal-title", "Password updated" }
                    p { class: "modal-copy",
                        "Your password has been reset. You can close this tab and log in with your new password."
                    }
                }
            }
        };
    }

    rsx! {
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        div { class: "modal-backdrop",
            div { class: "auth-panel modal-card",
                h2 { class: "modal-title", "Reset your password" }
                p { class: "modal-copy", "Choose a new password for your account." }

                input {
                    class: "auth-input",
                    r#type: "password",
                    placeholder: "New password",
                    value: "{new_password}",
                    oninput: move |event| new_password.set(event.value()),
                }
                input {
                    class: "auth-input",
                    r#type: "password",
                    placeholder: "Confirm new password",
                    value: "{confirm_password}",
                    oninput: move |event| confirm_password.set(event.value()),
                }

                if let Some(error) = error_message() {
                    p { class: "error-banner", "{error}" }
                }

                div { class: "modal-actions",
                    button {
                        class: "toggle-button",
                        disabled: is_submitting(),
                        onclick: move |_| {
                            let server_url = server_url.clone();
                            let token = token.clone();
                            let new_password_value = new_password();
                            let confirm_value = confirm_password();

                            if new_password_value.is_empty() {
                                error_message.set(Some("A new password is required".to_string()));
                                return;
                            }
                            if new_password_value != confirm_value {
                                error_message.set(Some("New password and confirmation don't match".to_string()));
                                return;
                            }

                            spawn(async move {
                                is_submitting.set(true);
                                error_message.set(None);
                                match crate::app::reset_password(&server_url, &token, &new_password_value).await {
                                    Ok(()) => succeeded.set(true),
                                    Err(error) => error_message.set(Some(error)),
                                }
                                is_submitting.set(false);
                            });
                        },
                        "Reset password"
                    }
                }
            }
        }
    }
}
