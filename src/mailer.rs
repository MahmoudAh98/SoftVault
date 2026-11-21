//! Sends mail through Gmail's SMTP server (smtp.gmail.com:465, implicit TLS)
//! using a Google *App Password*. The message is sent from your Gmail to the
//! same Gmail address – nothing goes to any third-party server.

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use std::time::Duration;

pub fn is_gmail(addr: &str) -> bool {
    let a = addr.trim().to_ascii_lowercase();
    let Some((user, domain)) = a.split_once('@') else { return false };
    !user.is_empty() && (domain == "gmail.com" || domain == "googlemail.com")
}

pub fn mask_email(addr: &str) -> String {
    match addr.split_once('@') {
        Some((u, d)) => {
            let first: String = u.chars().take(2).collect();
            format!("{first}•••@{d}")
        }
        None => "•••".into(),
    }
}

pub fn send_to_self(email: &str, app_password: &str, subject: &str, body: String) -> Result<(), String> {
    let addr = email.trim();
    let mbox: lettre::message::Mailbox =
        format!("SoftVault <{addr}>").parse().map_err(|e| format!("Invalid address: {e}"))?;
    let msg = Message::builder()
        .from(mbox.clone())
        .to(mbox)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| e.to_string())?;
    let creds = Credentials::new(addr.to_string(), app_password.replace(' ', ""));
    let mailer = SmtpTransport::relay("smtp.gmail.com")
        .map_err(|e| e.to_string())?
        .credentials(creds)
        .timeout(Some(Duration::from_secs(25)))
        .build();
    mailer.send(&msg).map(|_| ()).map_err(|e| {
        let s = e.to_string();
        if s.contains("535") || s.to_lowercase().contains("credentials") {
            "Gmail rejected the login. Use a 16-character Google App Password (requires 2-Step Verification on your Google account).".into()
        } else {
            format!("Could not send email: {s}")
        }
    })
}
