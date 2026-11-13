#![forbid(unsafe_code)]

// Register a real backend before enabling another target; do not build a fake
// "healthy" Agent which silently omits all OS-specific observations.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!(
    "EasyManage Agent currently supports Linux and Windows. The macOS backend is not implemented; see docs/architecture.md."
);

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod app;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod client;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod config;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod platform;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod store;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::ExitCode;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use tracing_subscriber::EnvFilter;

// A monitoring daemon does not need one async worker per host CPU.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[tokio::main(worker_threads = 2)]
async fn console_main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    #[cfg(windows)]
    if let Some(index) = args.iter().position(|arg| arg == "--diagnostics") {
        let Some(path) = args.get(index + 1) else {
            eprintln!("--diagnostics requires a new directory");
            return ExitCode::FAILURE;
        };
        return match platform::diagnostics(std::path::Path::new(path)).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("diagnostics failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(index) = args.iter().position(|arg| arg == "--config") {
        let Some(path) = args.get(index + 1) else {
            eprintln!("--config requires a path");
            return ExitCode::FAILURE;
        };
        if dotenvy::from_path(path).is_err() {
            eprintln!("cannot load specified Agent config");
            return ExitCode::FAILURE;
        }
    }
    initialize_crypto_provider();
    initialize_tracing();
    if let Err(error) = app::run().await {
        tracing::error!(error = %error, "agent stopped with an error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn initialize_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn initialize_tracing() {
    #[cfg(windows)]
    if std::env::args_os().any(|arg| arg == "--service") {
        if let Some(path) = std::env::var_os("EASYMANAGE_AGENT_STATE_DB")
            .map(std::path::PathBuf::from)
            .and_then(|p| {
                p.parent()
                    .and_then(std::path::Path::parent)
                    .map(|p| p.join("logs/agent.log"))
            })
        {
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
                let previous = path.with_extension("log.previous");
                let _ = std::fs::rename(&path, previous);
            }
            if let Ok(file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::new("easymanage_agent=info"))
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(file))
                    .init();
                return;
            }
        }
    }

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("easymanage_agent=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    console_main()
}
#[cfg(windows)]
mod windows_service;
#[cfg(windows)]
fn main() -> ExitCode {
    if std::env::args_os().any(|arg| arg == "--service") {
        return match windows_service::dispatch() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("service dispatcher failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    console_main()
}
