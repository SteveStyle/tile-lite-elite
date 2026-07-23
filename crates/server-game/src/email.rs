//! Minimal Resend client. Content lives in `crates/server-game/emails/*.txt`
//! — plain text, `{{placeholder}}` substitution, no conditionals or loops —
//! deliberately not a real templating engine, since these are all flat "hi
//! X, here's a link" emails and a template-engine dependency would be
//! solving a problem this project doesn't have. Editing the wording is a
//! content change to those files, not a code change here.

use serde_json::json;

#[derive(Clone)]
pub struct EmailConfig {
    /// `None` means no provider is configured (local dev, or the operator
    /// simply hasn't set `RESEND_API_KEY` yet) — `send` degrades to logging
    /// the full message instead of failing, so every email-triggering flow
    /// keeps working (and stays testable) with zero external dependency.
    api_key: Option<String>,
    from_address: String,
}

impl EmailConfig {
    pub fn new(api_key: Option<String>, from_address: String) -> Self {
        Self {
            api_key,
            from_address,
        }
    }
}

const WELCOME_TEMPLATE: &str = include_str!("../emails/welcome.txt");
const INVITATION_TEMPLATE: &str = include_str!("../emails/invitation.txt");
const JOIN_INVITATION_TEMPLATE: &str = include_str!("../emails/join-invitation.txt");
const PASSWORD_RESET_TEMPLATE: &str = include_str!("../emails/password-reset.txt");
const MOVE_REMINDER_TEMPLATE: &str = include_str!("../emails/move-reminder.txt");

pub async fn send_welcome(config: &EmailConfig, to: &str, display_name: &str, base_url: &str) {
    let (subject, body) = render(
        WELCOME_TEMPLATE,
        &[("display_name", display_name), ("base_url", base_url)],
    );
    send(config, to, &subject, &body).await;
}

pub async fn send_invitation(
    config: &EmailConfig,
    to: &str,
    invitee_name: &str,
    inviter_name: &str,
    base_url: &str,
) {
    let (subject, body) = render(
        INVITATION_TEMPLATE,
        &[
            ("invitee_name", invitee_name),
            ("inviter_name", inviter_name),
            ("base_url", base_url),
        ],
    );
    send(config, to, &subject, &body).await;
}

/// Unlike `send_invitation`, `to` has no known `Player` account behind it
/// yet — this is what `SeatClaim::Email` sends instead (see its doc
/// comment), a plain join link rather than "log in to accept".
pub async fn send_join_invitation(
    config: &EmailConfig,
    to: &str,
    inviter_name: &str,
    join_url: &str,
) {
    let (subject, body) = render(
        JOIN_INVITATION_TEMPLATE,
        &[("inviter_name", inviter_name), ("join_url", join_url)],
    );
    send(config, to, &subject, &body).await;
}

pub async fn send_password_reset(config: &EmailConfig, to: &str, reset_url: &str) {
    let (subject, body) = render(PASSWORD_RESET_TEMPLATE, &[("reset_url", reset_url)]);
    send(config, to, &subject, &body).await;
}

/// Sent at most once per turn, only once remaining time drops to a third
/// of the game's move-time-limit — see `app::send_move_time_reminders`.
/// `time_remaining` is a pre-formatted label like "1 day 4 hours".
pub async fn send_move_time_reminder(
    config: &EmailConfig,
    to: &str,
    display_name: &str,
    time_remaining: &str,
    base_url: &str,
) {
    let (subject, body) = render(
        MOVE_REMINDER_TEMPLATE,
        &[
            ("display_name", display_name),
            ("time_remaining", time_remaining),
            ("base_url", base_url),
        ],
    );
    send(config, to, &subject, &body).await;
}

/// Template format: a `Subject: ...` first line, a blank line, then the
/// plain-text body — everything after is verbatim aside from `{{key}}`
/// substitution (applied to both subject and body, since the invitation
/// email's subject line itself uses a placeholder).
fn render(template: &str, values: &[(&str, &str)]) -> (String, String) {
    let (subject_line, body) = template
        .split_once('\n')
        .expect("email template should have a Subject line, a blank line, then the body");
    let subject = subject_line
        .strip_prefix("Subject: ")
        .expect("email template's first line should read 'Subject: ...'");
    let body = body.trim_start_matches('\n');

    let substitute = |text: &str| {
        values.iter().fold(text.to_string(), |acc, (key, value)| {
            acc.replace(&format!("{{{{{key}}}}}"), value)
        })
    };
    (substitute(subject), substitute(body))
}

/// Fire-and-log, never fire-and-fail: a missing/failed send is always a
/// `warn`-level side note, never something that fails the caller's request.
/// Registering, inviting someone, or requesting a password reset should all
/// succeed on their own merits — none of them ought to depend on Resend
/// being reachable, same principle as everything else in this codebase that
/// treats a notification as best-effort rather than load-bearing.
async fn send(config: &EmailConfig, to: &str, subject: &str, text_body: &str) {
    let Some(api_key) = config.api_key.as_deref() else {
        // The email body is the only place this content exists when
        // there's no provider to deliver it — log it in full (rather than
        // just "would have sent something") so the flow stays usable and
        // testable in local dev with zero Resend setup.
        tracing::info!(
            to,
            subject,
            text_body,
            "email not sent (no RESEND_API_KEY configured)"
        );
        return;
    };

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.resend.com/emails")
        .bearer_auth(api_key)
        .json(&json!({
            "from": config.from_address,
            "to": [to],
            "subject": subject,
            "text": text_body,
        }))
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => {
            tracing::info!(to, subject, "email sent");
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(to, subject, %status, body, "email send failed");
        }
        Err(error) => {
            tracing::warn!(to, subject, %error, "email send failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_every_placeholder_in_subject_and_body() {
        let (subject, body) = render(
            "Subject: {{a}} says hi\n\nHello {{b}}, from {{a}}.\n",
            &[("a", "Alice"), ("b", "Bob")],
        );
        assert_eq!(subject, "Alice says hi");
        assert_eq!(body, "Hello Bob, from Alice.\n");
    }

    #[test]
    fn every_template_file_parses_and_leaves_no_placeholder_unfilled() {
        for (template, keys) in [
            (WELCOME_TEMPLATE, &["display_name", "base_url"][..]),
            (
                INVITATION_TEMPLATE,
                &["invitee_name", "inviter_name", "base_url"][..],
            ),
            (JOIN_INVITATION_TEMPLATE, &["inviter_name", "join_url"][..]),
            (PASSWORD_RESET_TEMPLATE, &["reset_url"][..]),
            (
                MOVE_REMINDER_TEMPLATE,
                &["display_name", "time_remaining", "base_url"][..],
            ),
        ] {
            let values: Vec<(&str, &str)> = keys.iter().map(|key| (*key, "x")).collect();
            let (subject, body) = render(template, &values);
            assert!(
                !subject.contains("{{") && !body.contains("{{"),
                "template left an unfilled {{{{placeholder}}}}: subject={subject:?} body={body:?}"
            );
        }
    }
}
