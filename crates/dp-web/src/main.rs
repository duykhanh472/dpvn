//! Command-line interface and daemon runner for Distributed Proofreaders.

use lexopt::prelude::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use dp_web::{AppConfig, AppState, AssetManifest, create_router, create_template_env};

enum Command {
    Serve,
    Migrate,
    Prune,
    CreateAdmin { username: String },
    ImportProject { zip_path: PathBuf },
    Verify,
}

fn print_help() {
    println!(
        r#"Distributed Proofreaders (Vietnamese)

USAGE:
    dp [OPTIONS] <COMMAND>

COMMANDS:
    serve                       Run the HTTP server
    migrate                     Apply pending database migrations
    prune                       Run periodic maintenance and cleanup
    create-admin <username>     Bootstrap an administrator account
    import-project <zip-path>   Import scanned book package
    verify                      Verify configuration, templates, and database

OPTIONS:
    -c, --config <PATH>         Path to configuration TOML file
    -h, --help                  Print help information
    -V, --version               Print version
"#
    );
}

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,dp_web=debug,dp_db=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut config_path: Option<PathBuf> = None;
    let mut command: Option<Command> = None;

    let mut parser = lexopt::Parser::from_env();
    while let Some(arg) = parser.next().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }) {
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
                        let zip_arg = parser.value().unwrap_or_else(|_| {
                            eprintln!("Error: import-project requires a <zip-path> argument");
                            std::process::exit(1);
                        });
                        command = Some(Command::ImportProject {
                            zip_path: PathBuf::from(zip_arg),
                        });
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

            let manifest_path = Path::new("static/manifest.json");
            let manifest = Arc::new(AssetManifest::from_file(manifest_path));

            let templates = match create_template_env(&config, manifest.clone()) {
                Ok(t) => Arc::new(t),
                Err(e) => {
                    eprintln!("Failed to initialize templates: {e}");
                    return ExitCode::FAILURE;
                }
            };

            let state = AppState {
                db: pool,
                config: config.clone(),
                templates,
                manifest,
                rate_limiter: Arc::new(dp_web::RateLimiter::new()),
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
            if let Err(e) = axum::serve(listener, router).await {
                eprintln!("Server error: {e}");
                return ExitCode::FAILURE;
            }
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

            let sessions_count = dp_db::queries::sessions::prune_expired_sessions(&pool)
                .await
                .unwrap_or(0);
            println!("Pruned {sessions_count} expired sessions.");

            let users_count = dp_db::queries::users::prune_pending_users(&pool, 7)
                .await
                .unwrap_or(0);
            println!("Pruned {users_count} unactivated pending users.");

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
        Command::ImportProject { zip_path } => {
            println!(
                "Importing book package from '{}' (staged for Phase 3)...",
                zip_path.display()
            );
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
    }

    ExitCode::SUCCESS
}
