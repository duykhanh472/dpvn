//! Outgoing email dispatch for account activation.

use lettre::{
    AsyncTransport, Message, Tokio1Executor, message::header::ContentType,
    transport::smtp::AsyncSmtpTransport,
};
use tracing::info;

use crate::app::AppError;

/// Dispatch an account activation email with an activation link.
///
/// # Errors
/// Returns [`AppError`] if message formulation fails.
pub async fn send_activation_email(
    smtp_url: &str,
    from_address: &str,
    public_url: &str,
    recipient_email: &str,
    username: &str,
    activate_token: &str,
) -> Result<(), AppError> {
    let activate_url = format!("{public_url}/activate/{activate_token}");

    let body = format!(
        "Chào {username},\n\n\
        Cảm ơn bạn đã đăng ký tham gia Distributed Proofreaders Việt Nam.\n\
        Vui lòng nhấp vào liên kết sau để kích hoạt tài khoản của bạn:\n\n\
        {activate_url}\n\n\
        Liên kết có hiệu lực trong 7 ngày.\n\n\
        Trân trọng,\n\
        Đội ngũ Distributed Proofreaders"
    );

    let email = Message::builder()
        .from(
            from_address
                .parse()
                .map_err(|e| AppError::Internal(format!("invalid from address: {e}")))?,
        )
        .to(recipient_email
            .parse()
            .map_err(|e| AppError::Internal(format!("invalid recipient address: {e}")))?)
        .subject("Kích hoạt tài khoản — Distributed Proofreaders")
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| AppError::Internal(format!("failed to construct email message: {e}")))?;

    // Attempt SMTP dispatch
    match AsyncSmtpTransport::<Tokio1Executor>::from_url(smtp_url) {
        Ok(transport) => {
            let mailer = transport.build();
            match mailer.send(email).await {
                Ok(_) => {
                    info!(
                        user = %username,
                        email = %recipient_email,
                        "activation email sent successfully via SMTP"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        activate_url = %activate_url,
                        "SMTP dispatch failed; fallback activation link logged for development"
                    );
                }
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                activate_url = %activate_url,
                "invalid SMTP URL; fallback activation link logged for development"
            );
        }
    }

    Ok(())
}
