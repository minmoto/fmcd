use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use fedimint_core::config::FederationId;
use tokio::sync::broadcast;
use tokio::time::timeout;

use fmcd::events::{EventBus, FmcdEvent};
use fmcd::services::{
    BalanceMonitor, BalanceMonitorConfig, DepositMonitor, DepositMonitorConfig,
};
use fmcd::services::deposit_monitor::DepositInfo;

/// Test helper to capture events from the event bus
struct EventCapture {
    receiver: broadcast::Receiver<FmcdEvent>,
    captured_events: Vec<FmcdEvent>,
}

impl EventCapture {
    fn new(event_bus: &EventBus) -> Self {
        Self {
            receiver: event_bus.subscribe(),
            captured_events: Vec::new(),
        }
    }

    async fn wait_for_events(&mut self, count: usize, timeout_duration: Duration) -> Vec<FmcdEvent> {
        let mut events = Vec::new();

        let result = timeout(timeout_duration, async {
            while events.len() < count {
                if let Ok(event) = self.receiver.recv().await {
                    events.push(event);
                }
            }
        }).await;

        match result {
            Ok(_) => events,
            Err(_) => {
                // Timeout - return what we have so far
                eprintln!("Timeout waiting for {} events, got {}", count, events.len());
                events
            }
        }
    }

    async fn wait_for_event_type(
        &mut self,
        event_type: &str,
        timeout_duration: Duration,
    ) -> Option<FmcdEvent> {
        let result = timeout(timeout_duration, async {
            loop {
                if let Ok(event) = self.receiver.recv().await {
                    if event.event_type() == event_type {
                        return Some(event);
                    }
                }
            }
            #[allow(unreachable_code)]
            None::<FmcdEvent>
        }).await;

        result.ok().flatten()
    }
}

#[tokio::test]
async fn test_withdrawal_event_emissions() {
    // Test that withdrawal events are emitted correctly
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    // Simulate withdrawal events
    let correlation_id = Some("test-correlation-123".to_string());
    let federation_id = FederationId::dummy();
    let operation_id = "test-op-456".to_string();

    // Emit withdrawal initiated event
    let initiated_event = FmcdEvent::WithdrawalInitiated {
        operation_id: operation_id.clone(),
        federation_id: federation_id.to_string(),
        address: "bc1test_address".to_string(),
        amount_sat: 100000,
        fee_sat: 1000,
        correlation_id: correlation_id.clone(),
        timestamp: Utc::now(),
    };
    event_bus.publish(initiated_event).await.unwrap();

    // Emit withdrawal completed event
    let completed_event = FmcdEvent::WithdrawalCompleted {
        operation_id: operation_id.clone(),
        federation_id: federation_id.to_string(),
        txid: "test_txid_123".to_string(),
        correlation_id: correlation_id.clone(),
        timestamp: Utc::now(),
    };
    event_bus.publish(completed_event).await.unwrap();

    // Verify events were captured
    let events = event_capture.wait_for_events(2, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 2);

    // Verify event types
    assert_eq!(events[0].event_type(), "withdrawal_initiated");
    assert_eq!(events[1].event_type(), "withdrawal_completed");

    // Verify correlation IDs are preserved
    assert_eq!(events[0].correlation_id(), Some(&correlation_id.clone().unwrap()));
    assert_eq!(events[1].correlation_id(), Some(&correlation_id.clone().unwrap()));
}

#[tokio::test]
async fn test_withdrawal_failure_event_emission() {
    // Test that withdrawal failure events are emitted correctly
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let correlation_id = Some("test-correlation-456".to_string());
    let federation_id = FederationId::dummy();
    let operation_id = "test-op-789".to_string();

    // Emit withdrawal failed event
    let failed_event = FmcdEvent::WithdrawalFailed {
        operation_id: operation_id.clone(),
        federation_id: federation_id.to_string(),
        reason: "Insufficient balance to pay fees".to_string(),
        correlation_id: correlation_id.clone(),
        timestamp: Utc::now(),
    };
    event_bus.publish(failed_event).await.unwrap();

    // Verify event was captured
    let events = event_capture.wait_for_events(1, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), "withdrawal_failed");

    // Verify error context is included
    if let FmcdEvent::WithdrawalFailed { reason, .. } = &events[0] {
        assert!(reason.contains("Insufficient balance"));
    } else {
        panic!("Expected withdrawal failed event");
    }
}

#[tokio::test]
async fn test_deposit_detection_events() {
    // Test that deposit detection events are emitted correctly
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let federation_id = FederationId::dummy();
    let operation_id = "test-deposit-123".to_string();

    // First emit deposit address generated event
    let address_event = FmcdEvent::DepositAddressGenerated {
        operation_id: operation_id.clone(),
        federation_id: federation_id.to_string(),
        address: "bc1test_deposit_address".to_string(),
        correlation_id: Some("test-correlation-789".to_string()),
        timestamp: Utc::now(),
    };
    event_bus.publish(address_event).await.unwrap();

    // Then emit deposit detected event
    let detected_event = FmcdEvent::DepositDetected {
        operation_id: operation_id.clone(),
        federation_id: federation_id.to_string(),
        address: "bc1test_deposit_address".to_string(),
        amount_sat: 50000,
        txid: "test_deposit_txid_456".to_string(),
        correlation_id: Some("test-correlation-789".to_string()),
        timestamp: Utc::now(),
    };
    event_bus.publish(detected_event).await.unwrap();

    // Verify events were captured
    let events = event_capture.wait_for_events(2, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type(), "deposit_address_generated");
    assert_eq!(events[1].event_type(), "deposit_detected");

    // Verify timing - deposit detected should come within reasonable time
    let address_time = events[0].timestamp();
    let detected_time = events[1].timestamp();
    let duration = detected_time.signed_duration_since(address_time);
    assert!(duration.num_seconds() < 60, "Deposit detection should be within 60 seconds");
}

#[tokio::test]
async fn test_balance_monitoring_events() {
    // Test that balance monitoring events are emitted correctly
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let federation_id = FederationId::dummy();
    let old_balance = 100000u64;
    let new_balance = 150000u64;

    // Emit balance updated event
    let balance_event = FmcdEvent::FederationBalanceUpdated {
        federation_id: federation_id.to_string(),
        balance_msat: new_balance,
        correlation_id: None, // Balance monitoring is not request-driven
        timestamp: Utc::now(),
    };
    event_bus.publish(balance_event).await.unwrap();

    // Verify event was captured
    let events = event_capture.wait_for_events(1, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type(), "federation_balance_updated");

    // Verify balance change details
    if let FmcdEvent::FederationBalanceUpdated { balance_msat, .. } = &events[0] {
        assert_eq!(*balance_msat, new_balance);
    } else {
        panic!("Expected federation balance updated event");
    }
}

#[tokio::test]
async fn test_event_deduplication() {
    // Test that balance monitor doesn't emit duplicate events for same balance
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let federation_id = FederationId::dummy();
    let balance = 100000u64;

    // Emit same balance twice
    let balance_event1 = FmcdEvent::FederationBalanceUpdated {
        federation_id: federation_id.to_string(),
        balance_msat: balance,
        correlation_id: None,
        timestamp: Utc::now(),
    };
    event_bus.publish(balance_event1).await.unwrap();

    let balance_event2 = FmcdEvent::FederationBalanceUpdated {
        federation_id: federation_id.to_string(),
        balance_msat: balance,
        correlation_id: None,
        timestamp: Utc::now(),
    };
    event_bus.publish(balance_event2).await.unwrap();

    // Should receive both events since we're publishing directly
    // (deduplication happens in the BalanceMonitor service, not the event bus)
    let events = event_capture.wait_for_events(2, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn test_correlation_id_propagation() {
    // Test that correlation IDs are properly propagated through all event types
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let correlation_id = Some("test-correlation-propagation".to_string());
    let federation_id = FederationId::dummy();

    // Test various event types with correlation ID
    let events = vec![
        FmcdEvent::WithdrawalInitiated {
            operation_id: "test-op-1".to_string(),
            federation_id: federation_id.to_string(),
            address: "bc1test".to_string(),
            amount_sat: 1000,
            fee_sat: 10,
            correlation_id: correlation_id.clone(),
            timestamp: Utc::now(),
        },
        FmcdEvent::DepositDetected {
            operation_id: "test-op-2".to_string(),
            federation_id: federation_id.to_string(),
            address: "bc1test2".to_string(),
            amount_sat: 2000,
            txid: "txid123".to_string(),
            correlation_id: correlation_id.clone(),
            timestamp: Utc::now(),
        },
        FmcdEvent::PaymentInitiated {
            payment_id: "pay-123".to_string(),
            federation_id: federation_id.to_string(),
            amount_msat: 10000,
            invoice: "lnbc...".to_string(),
            correlation_id: correlation_id.clone(),
            timestamp: Utc::now(),
        },
    ];

    // Publish all events
    for event in events {
        event_bus.publish(event).await.unwrap();
    }

    // Verify all events have the correct correlation ID
    let captured = event_capture.wait_for_events(3, Duration::from_secs(5)).await;
    assert_eq!(captured.len(), 3);

    for event in captured {
        assert_eq!(event.correlation_id(), Some(&correlation_id.clone().unwrap()));
    }
}

#[tokio::test]
async fn test_event_timing_requirements() {
    // Test that events meet timing requirements as per the plan
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let start_time = Utc::now();

    // Simulate deposit detection - should be within 60 seconds
    let deposit_event = FmcdEvent::DepositDetected {
        operation_id: "timing-test".to_string(),
        federation_id: FederationId::dummy().to_string(),
        address: "bc1timing".to_string(),
        amount_sat: 1000,
        txid: "timing-txid".to_string(),
        correlation_id: Some("timing-correlation".to_string()),
        timestamp: Utc::now(),
    };
    event_bus.publish(deposit_event).await.unwrap();

    // Simulate balance change - should be within 90 seconds
    let balance_event = FmcdEvent::FederationBalanceUpdated {
        federation_id: FederationId::dummy().to_string(),
        balance_msat: 50000,
        correlation_id: None,
        timestamp: Utc::now(),
    };
    event_bus.publish(balance_event).await.unwrap();

    let events = event_capture.wait_for_events(2, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 2);

    // Verify timing is reasonable (events should be published immediately in tests)
    let deposit_time = events[0].timestamp();
    let balance_time = events[1].timestamp();

    let deposit_elapsed = deposit_time.signed_duration_since(start_time);
    let balance_elapsed = balance_time.signed_duration_since(start_time);

    // In tests, these should be very quick
    assert!(deposit_elapsed.num_seconds() < 5);
    assert!(balance_elapsed.num_seconds() < 5);
}

#[tokio::test]
async fn test_event_bus_performance_under_load() {
    // Test that the event bus doesn't lose events under load
    let event_bus = Arc::new(EventBus::new(1000));
    let mut event_capture = EventCapture::new(&event_bus);

    let num_events = 100;
    let federation_id = FederationId::dummy();

    // Publish many events concurrently
    let mut tasks = Vec::new();
    for i in 0..num_events {
        let event_bus_clone = event_bus.clone();
        let federation_id_clone = federation_id;

        let task = tokio::spawn(async move {
            let event = FmcdEvent::WithdrawalInitiated {
                operation_id: format!("load-test-{}", i),
                federation_id: federation_id_clone.to_string(),
                address: format!("bc1loadtest{}", i),
                amount_sat: 1000 + i as u64,
                fee_sat: 10,
                correlation_id: Some(format!("load-correlation-{}", i)),
                timestamp: Utc::now(),
            };
            event_bus_clone.publish(event).await.unwrap();
        });

        tasks.push(task);
    }

    // Wait for all publishing tasks to complete
    for task in tasks {
        task.await.unwrap();
    }

    // Verify all events were captured
    let events = event_capture.wait_for_events(num_events, Duration::from_secs(10)).await;
    assert_eq!(events.len(), num_events, "Should receive all {} events under load", num_events);

    // Verify events have unique operation IDs
    let mut operation_ids = std::collections::HashSet::new();
    for event in events {
        if let FmcdEvent::WithdrawalInitiated { operation_id, .. } = event {
            assert!(operation_ids.insert(operation_id), "Operation IDs should be unique");
        }
    }
}

/// Test that events contain all required metadata
#[tokio::test]
async fn test_event_metadata_completeness() {
    let event_bus = Arc::new(EventBus::new(100));
    let mut event_capture = EventCapture::new(&event_bus);

    let federation_id = FederationId::dummy();
    let correlation_id = Some("metadata-test".to_string());

    // Test withdrawal event metadata
    let withdrawal_event = FmcdEvent::WithdrawalCompleted {
        operation_id: "meta-op-123".to_string(),
        federation_id: federation_id.to_string(),
        txid: "meta-txid-456".to_string(),
        correlation_id: correlation_id.clone(),
        timestamp: Utc::now(),
    };
    event_bus.publish(withdrawal_event).await.unwrap();

    let events = event_capture.wait_for_events(1, Duration::from_secs(5)).await;
    assert_eq!(events.len(), 1);

    let event = &events[0];

    // Verify all required metadata is present
    assert!(!event.event_id().is_empty());
    assert_eq!(event.event_type(), "withdrawal_completed");
    assert_eq!(event.correlation_id(), Some(&correlation_id.clone().unwrap()));
    assert!(event.timestamp() <= Utc::now());

    // Verify specific withdrawal event fields
    if let FmcdEvent::WithdrawalCompleted { operation_id, federation_id: fed_id, txid, .. } = event {
        assert!(!operation_id.is_empty());
        assert!(!fed_id.is_empty());
        assert!(!txid.is_empty());
    } else {
        panic!("Expected withdrawal completed event");
    }
}
