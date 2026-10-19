//! Running a helper program with a limit on how long it may take.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `command` and return its stdout if it succeeds within `limit`. Past
/// that it is killed: bluetoothctl with no bluetoothd, or a PipeWire that has
/// stopped answering, otherwise waits for ever.
pub fn output_within(command: &mut Command, limit: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on the side, or a chatty child fills the pipe and never exits.
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quick_command_answers() {
        let out = output_within(Command::new("echo").arg("hi"), Duration::from_secs(5));
        assert_eq!(out.as_deref(), Some("hi\n"));
    }

    #[test]
    fn a_hung_command_is_cut_off() {
        let started = Instant::now();
        let out = output_within(Command::new("sleep").arg("5"), Duration::from_millis(100));
        assert_eq!(out, None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_failure_is_none() {
        assert_eq!(
            output_within(&mut Command::new("false"), Duration::from_secs(5)),
            None
        );
    }
}
