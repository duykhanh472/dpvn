//! Application configuration loading, environment overrides, and startup validation.

use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use dp_core::DomainError;

/// Full application configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// HTTP server network settings.
    pub server: ServerConfig,
    /// MySQL database connection settings.
    pub database: DatabaseConfig,
    /// Filesystem paths for data and templates.
    pub paths: PathsConfig,
    /// Outgoing SMTP mail settings.
    pub mail: MailConfig,
    /// System operational limits.
    pub limits: LimitsConfig,
    /// Developer mode flag (hot reloads templates).
    #[serde(default)]
    pub dev_mode: bool,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("server", &self.server)
            .field("database", &"[REDACTED]")
            .field("paths", &self.paths)
            .field("mail", &"[REDACTED]")
            .field("limits", &self.limits)
            .field("dev_mode", &self.dev_mode)
            .finish()
    }
}

/// Server network configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Socket address or host:port to bind.
    pub bind: String,
    /// Public base URL (e.g. "https://proof.example.org").
    pub public_url: String,
}

/// Database connection and pooling configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Full MySQL DSN.
    pub url: String,
    /// Maximum connections in pool.
    pub max_connections: u32,
    /// Minimum idle connections in pool.
    pub min_connections: u32,
}

/// Filesystem paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    /// Local root for page images, display derivatives, and uploads.
    pub data: PathBuf,
    /// Optional directory for external minijinja templates (dev mode).
    #[serde(default)]
    pub templates: Option<PathBuf>,
}

/// Mail configuration.
#[derive(Clone, Serialize, Deserialize)]
pub struct MailConfig {
    /// SMTP connection URL.
    pub smtp_url: String,
    /// From email address header.
    pub from: String,
}

/// Operational limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Maximum allowed upload size in megabytes.
    pub max_upload_mb: u32,
    /// Daily proofing page limit per user.
    pub daily_pages: u32,
    /// Hours before an uncompleted page claim expires.
    pub claim_timeout_hours: u32,
}

impl AppConfig {
    /// Load configuration from a TOML file path and apply environment variable overrides.
    ///
    /// # Errors
    /// Returns [`DomainError::Config`] if the file cannot be read, parsed, or fails validation.
    pub fn load_from_file_or_default(path: Option<&Path>) -> Result<Self, DomainError> {
        let mut config: AppConfig = if let Some(p) = path {
            if p.exists() {
                let content = fs::read_to_string(p).map_err(|e| {
                    DomainError::Config(format!("failed to read config file {}: {e}", p.display()))
                })?;
                toml::from_str(&content).map_err(|e| {
                    DomainError::Config(format!("failed to parse TOML from {}: {e}", p.display()))
                })?
            } else {
                Self::default()
            }
        } else {
            Self::default()
        };

        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Override config fields from standard environment variables.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(url) = env::var("DP_DATABASE_URL") {
            self.database.url = url;
        }
        if let Ok(bind) = env::var("DP_BIND") {
            self.server.bind = bind;
        }
        if let Ok(public_url) = env::var("DP_PUBLIC_URL") {
            self.server.public_url = public_url;
        }
        if let Ok(smtp) = env::var("DP_SMTP_URL") {
            self.mail.smtp_url = smtp;
        }
        if let Ok(data) = env::var("DP_DATA_DIR") {
            self.paths.data = PathBuf::from(data);
        }
        if let Ok(tmpl) = env::var("DP_TEMPLATES_DIR") {
            self.paths.templates = Some(PathBuf::from(tmpl));
        }
        if let Ok(dev) = env::var("DP_DEV_MODE") {
            self.dev_mode = dev == "1" || dev.eq_ignore_ascii_case("true");
        }
        if let Ok(max_conn) = env::var("DP_MAX_CONNECTIONS") {
            if let Ok(n) = max_conn.parse::<u32>() {
                self.database.max_connections = n;
            }
        }
        if let Ok(min_conn) = env::var("DP_MIN_CONNECTIONS") {
            if let Ok(n) = min_conn.parse::<u32>() {
                self.database.min_connections = n;
            }
        }
    }

    /// Validate all configuration invariants.
    ///
    /// # Errors
    /// Returns [`DomainError::Config`] with a detailed description if any invariant is violated.
    pub fn validate(&self) -> Result<(), DomainError> {
        // Validate server bind
        if self.server.bind.trim().is_empty() {
            return Err(DomainError::Config(
                "server.bind cannot be empty".to_string(),
            ));
        }
        if self.server.bind.parse::<SocketAddr>().is_err() {
            // Check if it's host:port
            let parts: Vec<&str> = self.server.bind.rsplitn(2, ':').collect();
            if parts.len() != 2 || parts[0].parse::<u16>().is_err() {
                return Err(DomainError::Config(format!(
                    "invalid server.bind address: '{}'",
                    self.server.bind
                )));
            }
        }

        // Validate database URL
        if self.database.url.trim().is_empty() {
            return Err(DomainError::Config(
                "database.url cannot be empty".to_string(),
            ));
        }
        if !self.database.url.starts_with("mysql://") {
            return Err(DomainError::Config(
                "database.url must begin with 'mysql://'".to_string(),
            ));
        }

        // Validate pool bounds
        if self.database.max_connections == 0 {
            return Err(DomainError::Config(
                "database.max_connections must be at least 1".to_string(),
            ));
        }
        if self.database.min_connections > self.database.max_connections {
            return Err(DomainError::Config(format!(
                "database.min_connections ({}) cannot exceed max_connections ({})",
                self.database.min_connections, self.database.max_connections
            )));
        }

        // Validate paths
        if self.paths.data.as_os_str().is_empty() {
            return Err(DomainError::Config(
                "paths.data cannot be empty".to_string(),
            ));
        }

        // Validate limits
        if self.limits.max_upload_mb == 0 {
            return Err(DomainError::Config(
                "limits.max_upload_mb must be greater than 0".to_string(),
            ));
        }
        if self.limits.daily_pages == 0 {
            return Err(DomainError::Config(
                "limits.daily_pages must be greater than 0".to_string(),
            ));
        }
        if self.limits.claim_timeout_hours == 0 {
            return Err(DomainError::Config(
                "limits.claim_timeout_hours must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                bind: "127.0.0.1:8080".to_string(),
                public_url: "http://localhost:8080".to_string(),
            },
            database: DatabaseConfig {
                url: "mysql://dp:secret@127.0.0.1:3306/dp".to_string(),
                max_connections: 16,
                min_connections: 2,
            },
            paths: PathsConfig {
                data: PathBuf::from("./data"),
                templates: None,
            },
            mail: MailConfig {
                smtp_url: "smtp://localhost:25".to_string(),
                from: "no-reply@localhost".to_string(),
            },
            limits: LimitsConfig {
                max_upload_mb: 512,
                daily_pages: 100,
                claim_timeout_hours: 72,
            },
            dev_mode: false,
        }
    }
}
