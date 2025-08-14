#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;
    use tracing_appender::rolling::Rotation;

    use crate::observability::logging::LoggingConfig;

    #[test]
    fn test_logging_config_default() {
        let config = LoggingConfig::default();

        assert_eq!(config.level, "info");
        assert!(config.console_output);
        assert!(config.file_output);
        assert_eq!(config.log_dir, PathBuf::from("./logs"));
        assert_eq!(config.rotation, Rotation::DAILY);
        assert_eq!(config.file_permissions, 0o640);
        assert_eq!(config.max_log_files, Some(30));
    }

    #[test]
    fn test_logging_config_custom() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let log_dir = temp_dir.path().to_path_buf();

        let config = LoggingConfig {
            level: "debug".to_string(),
            console_output: false,
            file_output: true,
            log_dir: log_dir.clone(),
            rotation: Rotation::HOURLY,
            file_permissions: 0o600,
            max_log_files: Some(7),
        };

        assert_eq!(config.level, "debug");
        assert!(!config.console_output);
        assert!(config.file_output);
        assert_eq!(config.log_dir, log_dir);
        assert_eq!(config.rotation, Rotation::HOURLY);
        assert_eq!(config.file_permissions, 0o600);
        assert_eq!(config.max_log_files, Some(7));
    }

    #[test]
    fn test_logging_initialization() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let config = LoggingConfig {
            level: "trace".to_string(),
            console_output: false,
            file_output: true,
            log_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // This test just verifies the config can be created
        // Actual logging initialization would require a more complex test setup
        assert_eq!(config.level, "trace");
        assert!(!config.console_output);
        assert!(config.file_output);
    }
}
