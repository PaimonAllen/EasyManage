use std::io;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Execute only fixed, read-only inventory commands. Bound both time and output.
pub async fn read_command(program: &str, args: &[&str], limit: u64) -> io::Result<String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        stdout.take(limit + 1).read_to_end(&mut bytes).await?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(io::Error::other(
                "inventory command output exceeded safety limit",
            ));
        }
        if !child.wait().await?.success() {
            return Err(io::Error::other(
                "inventory command returned a failure status",
            ));
        }
        String::from_utf8(bytes)
            .map_err(|_| io::Error::other("inventory command returned invalid UTF-8"))
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
            Err(error)
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "inventory command timed out",
            ))
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::read_command;
    use std::io::ErrorKind;
    use std::time::{Duration, Instant};

    // Execute only inside the isolated test environment; production uses fixed inventory commands.
    #[tokio::test]
    async fn output_limit_rejects_truncated_inventory() {
        let error = read_command("/bin/sh", &["-c", "printf '12345678'"], 4)
            .await
            .expect_err("truncated inventory must never be reported as complete");
        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(error.to_string().contains("output exceeded safety limit"));
    }

    #[tokio::test]
    async fn exact_limit_output_is_accepted() {
        let output = read_command("/bin/sh", &["-c", "printf '1234'"], 4)
            .await
            .expect("an exact-length response is valid");
        assert_eq!(output, "1234");
    }

    #[tokio::test]
    async fn failed_command_does_not_expose_its_output() {
        let error = read_command(
            "/bin/sh",
            &[
                "-c",
                "printf 'private-stdout'; printf 'private-stderr' >&2; exit 7",
            ],
            1024,
        )
        .await
        .expect_err("nonzero inventory exit status must not yield metrics");
        assert!(error.to_string().contains("failure status"));
        assert!(!error.to_string().contains("private"));
    }

    #[tokio::test]
    async fn stalled_command_is_bounded_and_reports_timeout() {
        let started = Instant::now();
        let error = read_command("/bin/sleep", &["30"], 1024)
            .await
            .expect_err("a stalled driver/helper must not block collection indefinitely");
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(8));
    }
}
