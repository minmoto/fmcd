#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use crate::database::instrumented::*;
    use crate::events::EventBus;
    use crate::observability::correlation::RequestContext;

    // Mock database for testing
    #[derive(Default)]
    struct MockDatabase {
        data: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
        should_fail: bool,
    }

    impl MockDatabase {
        fn new_failing() -> Self {
            Self {
                data: Default::default(),
                should_fail: true,
            }
        }
    }

    #[async_trait]
    impl DatabaseInterface for MockDatabase {
        async fn get(&self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
            if self.should_fail {
                anyhow::bail!("Mock database failure");
            }
            Ok(self.data.lock().await.get(key).cloned())
        }

        async fn set(&self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
            if self.should_fail {
                anyhow::bail!("Mock database failure");
            }
            self.data.lock().await.insert(key.to_vec(), value.to_vec());
            Ok(())
        }

        async fn delete(&self, key: &[u8]) -> anyhow::Result<()> {
            if self.should_fail {
                anyhow::bail!("Mock database failure");
            }
            self.data.lock().await.remove(key);
            Ok(())
        }

        async fn exists(&self, key: &[u8]) -> anyhow::Result<bool> {
            if self.should_fail {
                anyhow::bail!("Mock database failure");
            }
            Ok(self.data.lock().await.contains_key(key))
        }

        async fn scan_prefix(&self, prefix: &[u8]) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
            if self.should_fail {
                anyhow::bail!("Mock database failure");
            }
            let data = self.data.lock().await;
            Ok(data
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect())
        }
    }

    #[tokio::test]
    async fn test_instrumented_database_success() {
        let event_bus = Arc::new(EventBus::new(100));
        let mock_db = MockDatabase::default();
        let instrumented_db = InstrumentedDatabase::new(mock_db, event_bus, "test");

        let key = b"test_key";
        let value = b"test_value";

        // Test set operation
        let result = instrumented_db.set(key, value).await;
        assert!(result.is_ok());

        // Test get operation
        let result = instrumented_db.get(key).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(value.to_vec()));

        // Test exists operation
        let result = instrumented_db.exists(key).await;
        assert!(result.is_ok());
        assert!(result.unwrap());

        // Check stats
        let stats = instrumented_db.stats().get_summary();
        assert_eq!(stats.total_operations, 3);
        assert_eq!(stats.successful_operations, 3);
        assert_eq!(stats.failed_operations, 0);
        assert_eq!(stats.success_rate, 100.0);
    }

    #[tokio::test]
    async fn test_instrumented_database_failure() {
        let event_bus = Arc::new(EventBus::new(100));
        let mock_db = MockDatabase::new_failing();
        let instrumented_db = InstrumentedDatabase::new(mock_db, event_bus, "test");

        let key = b"test_key";
        let value = b"test_value";

        // Test failing operations
        let result = instrumented_db.set(key, value).await;
        assert!(result.is_err());

        let result = instrumented_db.get(key).await;
        assert!(result.is_err());

        // Check stats
        let stats = instrumented_db.stats().get_summary();
        assert_eq!(stats.total_operations, 2);
        assert_eq!(stats.successful_operations, 0);
        assert_eq!(stats.failed_operations, 2);
        assert_eq!(stats.success_rate, 0.0);
    }

    #[tokio::test]
    async fn test_database_context_wrapper() {
        let event_bus = Arc::new(EventBus::new(100));
        let mock_db = MockDatabase::default();
        let instrumented_db = InstrumentedDatabase::new(mock_db, event_bus, "test");

        let context = RequestContext::new(Some("test_correlation".to_string()));
        let db_with_context = instrumented_db.with_context(&context);

        let key = b"test_key";
        let value = b"test_value";

        // Test operations with context
        let result = db_with_context.set(key, value).await;
        assert!(result.is_ok());

        let result = db_with_context.get(key).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some(value.to_vec()));
    }

    #[tokio::test]
    async fn test_key_prefix_generation() {
        let event_bus = Arc::new(EventBus::new(100));
        let mock_db = MockDatabase::default();
        let instrumented_db = InstrumentedDatabase::new(mock_db, event_bus, "test");

        // Test with short key
        let short_key = b"abc";
        let prefix = instrumented_db.get_key_prefix(short_key);
        assert_eq!(prefix, hex::encode(short_key));

        // Test with long key (should truncate to first 8 bytes)
        let long_key = b"0123456789abcdef";
        let prefix = instrumented_db.get_key_prefix(long_key);
        assert_eq!(prefix, hex::encode(&long_key[..8]));
    }

    #[tokio::test]
    async fn test_scan_prefix_operation() {
        let event_bus = Arc::new(EventBus::new(100));
        let mock_db = MockDatabase::default();
        let instrumented_db = InstrumentedDatabase::new(mock_db, event_bus, "test");

        // Set up test data
        let _ = instrumented_db.set(b"prefix_1", b"value_1").await;
        let _ = instrumented_db.set(b"prefix_2", b"value_2").await;
        let _ = instrumented_db.set(b"other_key", b"other_value").await;

        // Test prefix scan
        let result = instrumented_db.scan_prefix(b"prefix_").await;
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries.len(), 2);

        // Check stats
        let stats = instrumented_db.stats().get_summary();
        assert_eq!(stats.total_operations, 4); // 3 sets + 1 scan
        assert_eq!(stats.successful_operations, 4);
    }
}
