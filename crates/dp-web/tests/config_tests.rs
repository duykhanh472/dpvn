use dp_web::AppConfig;
use std::path::Path;

#[test]
fn test_default_config_valid() {
    let config = AppConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_bad_database_url() {
    let mut config = AppConfig::default();
    config.database.url = "postgres://localhost/db".to_string();
    let err = config.validate().unwrap_err();
    assert!(err.to_string().contains("must begin with 'mysql://'"));
}

#[test]
fn test_bad_database_connections() {
    let mut config = AppConfig::default();
    config.database.min_connections = 20;
    config.database.max_connections = 5;
    let err = config.validate().unwrap_err();
    assert!(err.to_string().contains("cannot exceed max_connections"));
}

#[test]
fn test_bad_server_bind() {
    let mut config = AppConfig::default();
    config.server.bind = "invalid-address-without-port".to_string();
    let err = config.validate().unwrap_err();
    assert!(err.to_string().contains("invalid server.bind address"));
}

#[test]
fn test_empty_data_path() {
    let mut config = AppConfig::default();
    config.paths.data = std::path::PathBuf::new();
    let err = config.validate().unwrap_err();
    assert!(err.to_string().contains("paths.data cannot be empty"));
}

#[test]
fn test_load_from_file_or_default() {
    // Non-existent file should fall back to default
    let non_existent = Path::new("non_existent_file_definitely.toml");
    let config = AppConfig::load_from_file_or_default(Some(non_existent)).unwrap();
    assert_eq!(config.server.bind, "127.0.0.1:8080");
}
