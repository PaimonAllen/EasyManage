//! Shared Windows shell discovery used by SSH management and the native Agent.
/// Discovery runs in built-in Windows PowerShell; actual work requires PowerShell 7+.
pub const WINDOWS_PWSH_DISCOVERY: &str = include_str!("pwsh-discovery.ps1");

#[cfg(windows)]
pub fn resolve_pwsh() -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::{
        io,
        path::PathBuf,
        process::{Command, Stdio},
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };
    static CACHE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    let mut cached = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| io::Error::other("PowerShell discovery cache unavailable"))?;
    if let Some(path) = cached.as_ref().filter(|path| path.is_file()) {
        return Ok(path.clone());
    }
    let hint = std::env::var("EASYMANAGE_PWSH_PATH")
        .unwrap_or_default()
        .replace('\'', "''");
    let machine = if std::env::args_os().any(|arg| arg == "--service") {
        "$true"
    } else {
        "$false"
    };
    let script = format!(
        "{WINDOWS_PWSH_DISCOVERY}\n[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false);[Console]::Write((Find-EasyManagePwsh '{hint}' {machine}))"
    );
    let host =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut child = Command::new(host)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PowerShell discovery timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output()?;
    let path = PathBuf::from(
        String::from_utf8(output.stdout)
            .map_err(|_| io::Error::other("PowerShell discovery returned invalid UTF-8"))?
            .trim(),
    );
    if !output.status.success() || !path.is_absolute() || !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "PowerShell 7+ not found for this account",
        ));
    }
    *cached = Some(path.clone());
    Ok(path)
}
