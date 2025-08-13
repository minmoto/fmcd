#![allow(clippy::unwrap_used)]

use axum::extract::ws::Message;
use serde_json::json;

// WebSocket message format tests

#[test]
fn test_websocket_request_message_format() {
    let request = json!({
        "method": "info",
        "params": {
            "federation_id": "dummy_federation"
        }
    });

    let message = Message::Text(serde_json::to_string(&request).unwrap());

    match message {
        Message::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed["method"], "info");
            assert!(parsed["params"].is_object());
        }
        _ => panic!("Expected text message"),
    }
}

#[test]
fn test_websocket_error_response_format() {
    let error_response = json!({
        "error": "Federation not found",
        "code": 404
    });

    let message = Message::Text(serde_json::to_string(&error_response).unwrap());

    match message {
        Message::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(parsed["error"].is_string());
            assert!(parsed["code"].is_number());
        }
        _ => panic!("Expected text message"),
    }
}

#[test]
fn test_websocket_success_response_format() {
    let success_response = json!({
        "result": {
            "federation_id": "dummy_federation",
            "balance": 1000
        }
    });

    let message = Message::Text(serde_json::to_string(&success_response).unwrap());

    match message {
        Message::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(parsed["result"].is_object());
            assert_eq!(parsed["result"]["balance"], 1000);
        }
        _ => panic!("Expected text message"),
    }
}

#[test]
fn test_websocket_close_message() {
    let close_msg = Message::Close(None);

    match close_msg {
        Message::Close(_) => {
            // Close message created successfully
        }
        _ => panic!("Expected close message"),
    }
}

#[test]
fn test_websocket_ping_pong() {
    let ping = Message::Ping(vec![1, 2, 3]);
    let pong = Message::Pong(vec![1, 2, 3]);

    match ping {
        Message::Ping(data) => assert_eq!(data, vec![1, 2, 3]),
        _ => panic!("Expected ping message"),
    }

    match pong {
        Message::Pong(data) => assert_eq!(data, vec![1, 2, 3]),
        _ => panic!("Expected pong message"),
    }
}

#[test]
fn test_websocket_binary_message() {
    let binary_data = vec![0u8, 1, 2, 3, 4, 5];
    let message = Message::Binary(binary_data.clone());

    match message {
        Message::Binary(data) => assert_eq!(data, binary_data),
        _ => panic!("Expected binary message"),
    }
}
