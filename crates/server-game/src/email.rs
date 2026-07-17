//! Minimal Resend client — one function, no retry/queue/template system.
//! Every call site (welcome, invitation notice, password reset) builds its
//! own subject/HTML and calls `send`; this hobby-scale app has no reason
//! for anything heavier than that yet.

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

/// Fire-and-log, never fire-and-fail: a missing/failed send is always a
/// `warn`-level side note, never something that fails the caller's request.
/// Registering, inviting someone, or requesting a password reset should all
/// succeed on their own merits — none of them ought to depend on Resend
/// being reachable, same principle as everything else in this codebase that
/// treats a notification as best-effort rather than load-bearing.
pub async fn send(config: &EmailConfig, to: &str, subject: &str, html_body: &str) {
    let Some(api_key) = config.api_key.as_deref() else {
        // The email body is the only place this content exists when
        // there's no provider to deliver it — log it in full (rather than
        // just "would have sent something") so the flow stays usable and
        // testable in local dev with zero Resend setup.
        tracing::info!(to, subject, html_body, "email not sent (no RESEND_API_KEY configured)");
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
            "html": html_body,
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
