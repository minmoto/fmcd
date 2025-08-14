#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{Method, StatusCode};
    use axum::middleware::Next;
    use axum::response::Response;
    use tower::ServiceExt;

    use crate::observability::correlation::{
        request_id_middleware, RequestContext, CORRELATION_ID_HEADER, REQUEST_ID_HEADER,
    };

    // Mock next middleware that extracts the context
    async fn mock_next(req: Request) -> Result<Response, StatusCode> {
        let context = req.extensions().get::<RequestContext>().cloned();

        if context.is_none() {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }

        Ok(Response::new(Body::empty()))
    }

    #[tokio::test]
    async fn test_request_id_middleware_with_correlation_id() {
        let mut request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header(CORRELATION_ID_HEADER, "test-correlation-123")
            .body(Body::empty())
            .expect("Failed to build test request");

        let next = Next::new(mock_next);
        let result = request_id_middleware(request, next).await;

        assert!(result.is_ok());
        let response = result.expect("Middleware should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        // Check headers are set
        let correlation_header = response
            .headers()
            .get(CORRELATION_ID_HEADER)
            .expect("Correlation ID header should be present");
        assert_eq!(correlation_header, "test-correlation-123");

        let request_header = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("Request ID header should be present");
        assert!(!request_header.is_empty());
    }

    #[tokio::test]
    async fn test_request_id_middleware_without_correlation_id() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/test")
            .body(Body::empty())
            .expect("Failed to build test request");

        let next = Next::new(mock_next);
        let result = request_id_middleware(request, next).await;

        assert!(result.is_ok());
        let response = result.expect("Middleware should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        // Both headers should be present
        let correlation_header = response
            .headers()
            .get(CORRELATION_ID_HEADER)
            .expect("Correlation ID should be generated");
        let request_header = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("Request ID should be generated");

        assert!(!correlation_header.is_empty());
        assert!(!request_header.is_empty());
        // They should be different values
        assert_ne!(correlation_header, request_header);
    }

    #[tokio::test]
    async fn test_invalid_correlation_id_rejected() {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header(CORRELATION_ID_HEADER, "invalid-id-with-@#$%")
            .body(Body::empty())
            .expect("Failed to build test request");

        let next = Next::new(mock_next);
        let result = request_id_middleware(request, next).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_too_long_correlation_id_rejected() {
        let long_id = "a".repeat(201); // Exceeds MAX_CORRELATION_ID_LENGTH
        let request = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header(CORRELATION_ID_HEADER, long_id)
            .body(Body::empty())
            .expect("Failed to build test request");

        let next = Next::new(mock_next);
        let result = request_id_middleware(request, next).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StatusCode::BAD_REQUEST);
    }
}
