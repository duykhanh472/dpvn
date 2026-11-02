//! Command-line interface and daemon runner for Distributed Proofreaders.

use lexopt::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use dp_web::{AppConfig, AppState, AssetManifest, create_router, create_template_env};

enum Command {
    Serve,
    Migrate,
    Prune,
    CreateAdmin {
        username: String,
    },
    ImportProject {
        zip_path: PathBuf,
        name: String,
        author: String,
        tone_style: String,
        replace: bool,
    },
    Verify,
    Healthcheck {
        url: Option<String>,
    },
}

fn print_help() {
    println!(
        r#"Distributed Proofreaders (Vietnamese)

USAGE:
    dp [OPTIONS] <COMMAND>

COMMANDS:
    serve                            Run the HTTP server
    migrate                          Apply pending database migrations
    prune                            Run periodic maintenance and cleanup
    create-admin <username>          Bootstrap an administrator account
    import-project <zip-path>        Import scanned book package
    verify                           Verify configuration, templates, and database
    healthcheck [--url <URL>]        Probe /healthz (used by Docker HEALTHCHECK)

healthcheck OPTIONS:
    --url <URL>                      Probe URL (default: http://127.0.0.1:<bind>/healthz)

import-project OPTIONS:
    --name <TITLE>                   Book title (required)
    --author <AUTHOR>                Book author (required)
    --tone-style <old|new|either>    Tone mark style (default: either)
    --replace                        Replace existing pages if project already has data

OPTIONS:
    -c, --config <PATH>              Path to configuration TOML file
    -h, --help                       Print help information
    -V, --version                    Print version
"#
    );
}

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing. `DP_LOG_FORMAT=json` emits one JSON object per line,
    // which is what container platforms expect (§21.4).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,dp_web=info,dp_db=warn".into());
    let registry = tracing_subscriber::registry().with(filter);
    if std::env::var("DP_LOG_FORMAT").is_ok_and(|value| value.eq_ignore_ascii_case("json")) {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(true),
            )
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }

    let mut config_path: Option<PathBuf> = None;
    let mut command: Option<Command> = None;

    let mut parser = lexopt::Parser::from_env();
    #[allow(clippy::while_let_loop)]
    loop {
        let Some(arg) = parser.next().unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }) else {
            break;
        };

        match arg {
            Short('c') | Long("config") => {
                let val = parser.value().unwrap_or_else(|e| {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                });
                config_path = Some(PathBuf::from(val));
            }
            Short('h') | Long("help") => {
                print_help();
                return ExitCode::SUCCESS;
            }
            Short('V') | Long("version") => {
                println!("dp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Value(val) => {
                let cmd_str = val.string().unwrap_or_default();
                match cmd_str.as_str() {
                    "serve" => command = Some(Command::Serve),
                    "migrate" => command = Some(Command::Migrate),
                    "prune" => command = Some(Command::Prune),
                    "verify" => command = Some(Command::Verify),
                    "healthcheck" => {
                        let mut url = None;
                        while let Some(flag) = parser.next().unwrap_or(None) {
                            match flag {
                                Long("url") => {
                                    url = Some(
                                        parser
                                            .value()
                                            .unwrap_or_else(|_| {
                                                eprintln!("Error: --url requires a value");
                                                std::process::exit(1);
                                            })
                                            .string()
                                            .unwrap_or_default(),
                                    );
                                }
                                other => {
                                    eprintln!("Error: unexpected flag {other:?}");
                                    return ExitCode::FAILURE;
                                }
                            }
                        }
                        command = Some(Command::Healthcheck { url });
                        break;
                    }
                    "create-admin" => {
                        let user_arg = parser.value().unwrap_or_else(|_| {
                            eprintln!("Error: create-admin requires a <username> argument");
                            std::process::exit(1);
                        });
                        command = Some(Command::CreateAdmin {
                            username: user_arg.string().unwrap_or_default(),
                        });
                    }
                    "import-project" => {
                        // Positional: zip path
                        let zip_arg = parser.value().unwrap_or_else(|_| {
                            eprintln!("Error: import-project requires a <zip-path> argument");
                            std::process::exit(1);
                        });
                        let zip_path = PathBuf::from(zip_arg);

                        // Parse remaining flags for import-project
                        let mut name = String::new();
                        let mut author = String::new();
                        let mut tone_style = "either".to_owned();
                        let mut replace = false;

                        while let Some(flag) = parser.next().unwrap_or(None) {
                            match flag {
                                Long("name") => {
                                    name = parser
                                        .value()
                                        .unwrap_or_else(|_| {
                                            eprintln!("Error: --name requires a value");
                                            std::process::exit(1);
                                        })
                                        .string()
                                        .unwrap_or_default();
                                }
                                Long("author") => {
                                    author = parser
                                        .value()
                                        .unwrap_or_else(|_| {
                                            eprintln!("Error: --author requires a value");
                                            std::process::exit(1);
                                        })
                                        .string()
                                        .unwrap_or_default();
                                }
                                Long("tone-style") => {
                                    let ts = parser
                                        .value()
                                        .unwrap_or_else(|_| {
                                            eprintln!("Error: --tone-style requires a value");
                                            std::process::exit(1);
                                        })
                                        .string()
                                        .unwrap_or_default();
                                    match ts.as_str() {
                                        "old" | "new" | "either" => tone_style = ts,
                                        other => {
                                            eprintln!(
                                                "Error: --tone-style phải là 'old', 'new', hoặc 'either' (nhận: '{other}')"
                                            );
                                            return ExitCode::FAILURE;
                                        }
                                    }
                                }
                                Long("replace") => replace = true,
                                other => {
                                    eprintln!("Error: unexpected flag {other:?}");
                                    return ExitCode::FAILURE;
                                }
                            }
                        }

                        command = Some(Command::ImportProject {
                            zip_path,
                            name,
                            author,
                            tone_style,
                            replace,
                        });
                        break;
                    }
                    other => {
                        eprintln!("Error: unknown command '{other}'\n");
                        print_help();
                        return ExitCode::FAILURE;
                    }
                }
                break;
            }
            _ => {
                eprintln!("Error: unexpected argument\n");
                print_help();
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(cmd) = command else {
        print_help();
        return ExitCode::FAILURE;
    };

    // Load configuration
    let default_config = Path::new("config.toml");
    let resolved_path = config_path.as_deref().or(if default_config.exists() {
        Some(default_config)
    } else {
        None
    });

    let config = match AppConfig::load_from_file_or_default(resolved_path) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("Configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };

    match cmd {
        Command::Serve => {
            tracing::info!(bind = %config.server.bind, "initializing database connection pool");
            let pool = match dp_db::create_pool(
                &config.database.url,
                config.database.min_connections,
                config.database.max_connections,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Failed to connect to database: {e}");
                    return ExitCode::FAILURE;
                }
            };

            let manifest_path = std::env::var("DP_MANIFEST_PATH")
                .unwrap_or_else(|_| "static/manifest.json".to_string());
            let manifest = Arc::new(AssetManifest::from_file(Path::new(&manifest_path)));

            let templates = match create_template_env(&config, manifest.clone()) {
                Ok(t) => Arc::new(t),
                Err(e) => {
                    eprintln!("Failed to initialize templates: {e}");
                    return ExitCode::FAILURE;
                }
            };

            let site_words_data = dp_db::queries::words::get_all_site_words(&pool)
                .await
                .unwrap_or_default();

            let state = AppState {
                db: pool,
                config: config.clone(),
                templates,
                manifest,
                rate_limiter: Arc::new(dp_web::RateLimiter::new()),
                site_words: Arc::new(tokio::sync::RwLock::new(site_words_data)),
            };

            let router = create_router(state);

            let listener = match TcpListener::bind(&config.server.bind).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Failed to bind to {}: {e}", config.server.bind);
                    return ExitCode::FAILURE;
                }
            };

            tracing::info!(bind = %config.server.bind, "server listening for requests");
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal())
                .await
            {
                eprintln!("Server error: {e}");
                return ExitCode::FAILURE;
            }
            tracing::info!("server stopped cleanly");
        }
        Command::Migrate => {
            println!("Connecting to database for migration...");
            let pool =
                match dp_db::create_pool(&config.database.url, 1, config.database.max_connections)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Failed to connect to database: {e}");
                        return ExitCode::FAILURE;
                    }
                };

            match dp_db::run_migrations(&pool).await {
                Ok(()) => println!("Migrations applied successfully."),
                Err(e) => {
                    eprintln!("Migration failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::Prune => {
            println!("Running prune maintenance tasks...");
            let pool =
                match dp_db::create_pool(&config.database.url, 1, config.database.max_connections)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Failed to connect to database: {e}");
                        return ExitCode::FAILURE;
                    }
                };

            let site_settings = dp_web::SiteSettings::load(&pool, &config.limits)
                .await
                .unwrap_or_else(|_| dp_web::SiteSettings::from_defaults(&config.limits));

            let result = dp_web::jobs::prune::run_all(
                &pool,
                &config.paths.data.to_string_lossy(),
                24, // orphan_age_hours
                7,  // pending_user_days
                site_settings.claim_timeout_hours,
            )
            .await;

            println!("Pruned {} expired sessions.", result.sessions_removed);
            println!(
                "Pruned {} unactivated pending users.",
                result.pending_users_removed
            );
            println!(
                "Released {} stale page claims.",
                result.stale_claims_released
            );
            println!("Pruned {} tmp files.", result.tmp_files_removed);
            println!(
                "Pruned {} orphan image files.",
                result.orphan_images_removed
            );
            println!(
                "Pruned {} orphan display files.",
                result.orphan_display_removed
            );
            println!("Prune completed.");
        }
        Command::CreateAdmin { username } => {
            let password = std::env::var("DP_ADMIN_PASSWORD").unwrap_or_else(|_| {
                use std::io::{self, BufRead, Write};
                print!("Enter admin password: ");
                let _ = io::stdout().flush();
                let stdin = io::stdin();
                stdin
                    .lock()
                    .lines()
                    .next()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
            });

            if let Err(e) = dp_web::auth::validate_password(&password) {
                eprintln!("Invalid password: {e}");
                return ExitCode::FAILURE;
            }

            let pool =
                match dp_db::create_pool(&config.database.url, 1, config.database.max_connections)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Failed to connect to database: {e}");
                        return ExitCode::FAILURE;
                    }
                };

            let hash = match dp_web::auth::hash_password(password).await {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("Failed to hash password: {e}");
                    return ExitCode::FAILURE;
                }
            };

            let email = format!("{username}@example.com");
            match dp_db::queries::users::create_admin_user(&pool, &username, &email, &hash).await {
                Ok(id) => println!("Admin user '{username}' created with ID {id}."),
                Err(e) => {
                    eprintln!("Failed to create admin user: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::ImportProject {
            zip_path,
            name,
            author,
            tone_style,
            replace,
        } => {
            // Validate required args
            if name.is_empty() {
                eprintln!("Error: --name là bắt buộc cho lệnh import-project");
                return ExitCode::FAILURE;
            }
            if author.is_empty() {
                eprintln!("Error: --author là bắt buộc cho lệnh import-project");
                return ExitCode::FAILURE;
            }

            // Read zip bytes
            let zip_bytes = match tokio::fs::read(&zip_path).await {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "Error: không đọc được tệp zip '{}': {e}",
                        zip_path.display()
                    );
                    return ExitCode::FAILURE;
                }
            };

            println!(
                "Phân tích gói '{}' ({} byte)...",
                zip_path.display(),
                zip_bytes.len()
            );

            // Dry run preview
            let preview = match dp_web::ingest::dry_run(&zip_bytes).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Lỗi phân tích gói: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Print preview
            println!();
            println!("=== Xem trước nhập dữ liệu ===");
            println!("  Tổng số trang: {}", preview.total_pages);
            println!("  Trang có cảnh báo: {}", preview.warning_pages);
            println!("  Số lỗi: {}", preview.error_count);

            if !preview.global_warnings.is_empty() {
                println!();
                println!("Cảnh báo:");
                for w in &preview.global_warnings {
                    println!("  ⚠  {w}");
                }
            }

            if preview.error_count > 0 {
                println!();
                eprintln!(
                    "Nhập thất bại: có {} lỗi cần sửa trước khi nhập.",
                    preview.error_count
                );
                return ExitCode::FAILURE;
            }

            if !preview.can_commit() {
                println!();
                eprintln!(
                    "Nhập bị từ chối: không thể commit (lỗi = {}, trang = {}).",
                    preview.error_count, preview.total_pages
                );
                return ExitCode::FAILURE;
            }

            // Connect DB and commit
            let pool =
                match dp_db::create_pool(&config.database.url, 1, config.database.max_connections)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Failed to connect to database: {e}");
                        return ExitCode::FAILURE;
                    }
                };

            // Create the project record (CLI uses admin user_id=1 as manager)
            let project_id = match dp_db::queries::projects::create_project(
                &pool,
                &dp_db::queries::projects::NewProject {
                    name: &name,
                    author: &author,
                    tone_style: &tone_style,
                    charset: "vi",
                    manager_id: 1,
                    image_source: "",
                    comments: "",
                },
            )
            .await
            {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("Lỗi tạo dự án: {e}");
                    return ExitCode::FAILURE;
                }
            };

            println!();
            println!("Đã tạo dự án #{project_id} '{name}'. Đang nhập trang...");

            match dp_web::ingest::commit(
                &pool,
                &zip_bytes,
                project_id,
                &config.paths.data.to_string_lossy(),
                replace,
            )
            .await
            {
                Ok(inserted) => {
                    println!(
                        "✓ Nhập thành công: {} trang vào dự án #{project_id}.",
                        inserted.len()
                    );
                }
                Err(e) => {
                    eprintln!("Lỗi nhập dữ liệu: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::Verify => {
            println!("Verifying system configuration and templates...");
            let manifest = Arc::new(AssetManifest::default());
            match create_template_env(&config, manifest) {
                Ok(env) => {
                    let count = env.templates().count();
                    println!("Templates verified ({count} templates loaded).");
                }
                Err(e) => {
                    eprintln!("Template verification failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
            println!("Configuration OK.");
        }
        Command::Healthcheck { url } => {
            let target = url
                .or_else(|| std::env::var("DP_HEALTHCHECK_URL").ok())
                .unwrap_or_else(|| default_healthcheck_url(&config.server.bind));

            match http_get_ok(&target).await {
                Ok(()) => {
                    println!("OK {target}");
                }
                Err(e) => {
                    eprintln!("Healthcheck failed for {target}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    ExitCode::SUCCESS
}

/// Resolve when the process receives SIGINT or SIGTERM.
///
/// Graceful shutdown lets in-flight saves finish and the connection pool close
/// cleanly during a rolling deploy (§21.3).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }

    tracing::info!("shutdown signal received; draining in-flight requests");
}

/// Build the default probe URL from the configured bind address.
fn default_healthcheck_url(bind: &str) -> String {
    let port = bind.rsplit(':').next().unwrap_or("8080");
    if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]:") || bind.starts_with(":::") {
        format!("http://127.0.0.1:{port}/healthz")
    } else {
        format!("http://{bind}/healthz")
    }
}

/// Parse an `http://host[:port][/path]` URL without pulling in an HTTP client.
fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "only http:// URLs are supported".to_string())?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err("missing host".to_string());
    }

    let (host, port) = if let Some(after_bracket) = authority.strip_prefix('[') {
        let (host, remainder) = after_bracket
            .split_once(']')
            .ok_or_else(|| "invalid IPv6 authority".to_string())?;
        let port = match remainder.strip_prefix(':') {
            Some(value) => value
                .parse::<u16>()
                .map_err(|_| "invalid port".to_string())?,
            None => 80,
        };
        (host.to_string(), port)
    } else if let Some((host, value)) = authority.rsplit_once(':') {
        (
            host.to_string(),
            value
                .parse::<u16>()
                .map_err(|_| "invalid port".to_string())?,
        )
    } else {
        (authority.to_string(), 80)
    };

    Ok((host, port, path.to_string()))
}

/// Perform a minimal HTTP/1.1 GET and succeed only on a 200 response.
async fn http_get_ok(url: &str) -> Result<(), String> {
    let (host, port, path) = parse_http_url(url)?;
    let connect = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| "connection timed out".to_string())?
    .map_err(|e| format!("connection failed: {e}"))?;

    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    let exchange = async {
        let mut stream = connect;
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| format!("write failed: {e}"))?;
        let mut response = Vec::with_capacity(512);
        stream
            .read_to_end(&mut response)
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        Ok::<Vec<u8>, String>(response)
    };

    let response = tokio::time::timeout(Duration::from_secs(5), exchange)
        .await
        .map_err(|_| "response timed out".to_string())??;
    let head = String::from_utf8_lossy(&response);
    let status_line = head.lines().next().unwrap_or_default();
    if status_line.starts_with("HTTP/1.1 200") || status_line.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        Err(format!("unexpected response: {status_line}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_urls() {
        assert_eq!(
            parse_http_url("http://127.0.0.1:8080/healthz").unwrap(),
            ("127.0.0.1".to_string(), 8080, "/healthz".to_string())
        );
        assert_eq!(
            parse_http_url("http://example.test").unwrap(),
            ("example.test".to_string(), 80, "/".to_string())
        );
        assert_eq!(
            parse_http_url("http://[::1]:9000/readyz").unwrap(),
            ("::1".to_string(), 9000, "/readyz".to_string())
        );
    }

    #[test]
    fn rejects_non_http_urls() {
        assert!(parse_http_url("https://example.test/healthz").is_err());
    }

    #[test]
    fn default_probe_url_rewrites_wildcard_binds() {
        assert_eq!(
            default_healthcheck_url("0.0.0.0:8080"),
            "http://127.0.0.1:8080/healthz"
        );
        assert_eq!(
            default_healthcheck_url("127.0.0.1:9000"),
            "http://127.0.0.1:9000/healthz"
        );
    }
}
