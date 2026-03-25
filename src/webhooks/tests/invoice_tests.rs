#![allow(clippy::unwrap_used)]

use chrono::Utc;
use serde_json::json;

use crate::webhooks::invoice::InvoiceWebhookEvent;

#[tokio::test]
async fn test_webhook_event_serialization() {
    let event = InvoiceWebhookEvent {
        event_type: "invoice_paid".to_string(),
        operation_id: "test-op-123".to_string(),
        invoice_id: "test-invoice-456".to_string(),
        federation_id: "test-fed-789".to_string(),
        timestamp: Utc::now(),
        data: json!({
            "amount_received_msat": 1000000,
            "settled_at": "2024-01-01T12:00:00Z"
        }),
    };

    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains("invoice_paid"));
    assert!(serialized.contains("test-op-123"));
    assert!(serialized.contains("1000000"));
}
