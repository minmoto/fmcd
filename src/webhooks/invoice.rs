use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::{error, info, warn};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceWebhookEvent {
    pub event_type: String,
    pub operation_id: String,
    pub invoice_id: String,
    pub federation_id: String,
    pub timestamp: DateTime<Utc>,
    pub data: serde_json::Value,
}

/// Send webhook notification for invoice events
/// Uses exponential backoff with retries for reliability
pub async fn send_invoice_webhook(
    webhook_url: &str,
    event: &InvoiceWebhookEvent,
) -> anyhow::Result<()> {
    const MAX_RETRIES: u32 = 3;
    const BASE_DELAY: Duration = Duration::from_millis(100);
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    for attempt in 0..MAX_RETRIES {
        // Use bit shifting for efficient exponential backoff: 2^n = 1 << n
        let delay = BASE_DELAY * (1 << attempt);

        if attempt > 0 {
            info!(
                webhook_url = %webhook_url,
                attempt = attempt + 1,
                delay_ms = delay.as_millis(),
                "Retrying webhook request after delay"
            );
            tokio::time::sleep(delay).await;
        }

        match send_webhook_request(&client, webhook_url, event).await {
            Ok(()) => {
                info!(
                    webhook_url = %webhook_url,
                    event_type = %event.event_type,
                    operation_id = %event.operation_id,
                    attempt = attempt + 1,
                    "Webhook sent successfully"
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    webhook_url = %webhook_url,
                    event_type = %event.event_type,
                    operation_id = %event.operation_id,
                    attempt = attempt + 1,
                    error = ?e,
                    "Webhook request failed"
                );

                if attempt == MAX_RETRIES - 1 {
                    error!(
                        webhook_url = %webhook_url,
                        event_type = %event.event_type,
                        operation_id = %event.operation_id,
                        max_retries = MAX_RETRIES,
                        "Webhook delivery failed after all retries"
                    );
                    return Err(anyhow::anyhow!(
                        "Webhook delivery failed after {} retries: {}",
                        MAX_RETRIES,
                        e
                    ));
                }
            }
        }
    }

    Ok(())
}

async fn send_webhook_request(
    client: &reqwest::Client,
    webhook_url: &str,
    event: &InvoiceWebhookEvent,
) -> anyhow::Result<()> {
    let response = client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "fmcd-webhook/1.0")
        .header("X-Fmcd-Event-Type", &event.event_type)
        .header("X-Fmcd-Operation-Id", &event.operation_id)
        .header("X-Fmcd-Timestamp", event.timestamp.to_rfc3339())
        .json(event)
        .send()
        .await?;

    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!(
            "Webhook request failed with status {}: {}",
            status,
            error_body
        );
    }
}
