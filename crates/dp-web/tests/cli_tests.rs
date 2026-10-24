use std::process::Command;

#[test]
fn test_cli_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_dp"))
        .arg("--help")
        .output()
        .expect("failed to execute dp");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Distributed Proofreaders"));
    assert!(stdout.contains("serve"));
    assert!(stdout.contains("migrate"));
}

#[test]
fn test_cli_bad_config_exits_with_error() {
    // Create temporary bad config
    let temp_dir = std::env::temp_dir();
    let bad_config_path = temp_dir.join("dp_test_bad_config.toml");
    std::fs::write(
        &bad_config_path,
        r#"
[server]
bind = "invalid_bind_no_port"
public_url = "http://localhost"

[database]
url = "postgres://not-mysql"
max_connections = 10
min_connections = 2

[paths]
data = "./data"

[mail]
smtp_url = "smtp://localhost:25"
from = "test@example.com"

[limits]
max_upload_mb = 10
daily_pages = 10
claim_timeout_hours = 24
"#,
    )
    .expect("failed to write temp bad config");

    let output = Command::new(env!("CARGO_BIN_EXE_dp"))
        .args(["-c", bad_config_path.to_str().unwrap(), "serve"])
        .output()
        .expect("failed to execute dp");

    // Must exit with error
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Configuration error:"),
        "stderr did not contain expected error: {stderr}"
    );

    let _ = std::fs::remove_file(bad_config_path);
}
