//! The backlight through the keyboard's RP2040, which owns the PWM pin.
//!
//! Linux has no `/sys/class/backlight` on this board. The patched firmware
//! listens on its USB console for `B<0-100>` and `B?` and answers
//! `PT35 B=<percent>`. Stock firmware ignores the lines, the reply never
//! comes, and brightness stays what it was: the Fn keys on the keyboard.

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SerialBacklight {
    pub path: PathBuf,
    /// The level the firmware last reported. `None` until it has answered.
    pub level: Option<u8>,
}

/// What asking the keyboard got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Level(u8),
    /// No reply: stock firmware, or no console at all.
    Silent,
    /// `pt35-kbd` holds the port. Says nothing about the firmware.
    Busy,
}

/// The console device node. A replug makes a new one, so a change in it is a
/// keyboard worth asking again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleId {
    path: PathBuf,
    inode: u64,
    changed: i64,
}

impl ConsoleId {
    pub fn now() -> Option<Self> {
        let path = console()?;
        let meta = std::fs::metadata(&path).ok()?;
        Some(Self {
            path,
            inode: meta.ino(),
            changed: meta.ctime(),
        })
    }
}

impl SerialBacklight {
    /// The keyboard's console, if it is plugged in, asked for its level once.
    pub fn find() -> (Option<Self>, Answer) {
        let Some(path) = console() else {
            return (None, Answer::Silent);
        };
        let mut backlight = Self { path, level: None };
        let answer = backlight.read();
        (backlight.level.is_some().then_some(backlight), answer)
    }

    /// Ask for the level, which Fn - and = change behind our back.
    pub fn read(&mut self) -> Answer {
        let answer = ask(&self.path, "B?");
        if let Answer::Level(level) = answer {
            self.level = Some(level);
        }
        answer
    }

    /// Set a level; returns what the firmware says it is now.
    pub fn set(&mut self, percent: u8) -> Option<u8> {
        let Answer::Level(level) = ask(&self.path, &format!("B{}", percent.min(100))) else {
            return None;
        };
        self.level = Some(level);
        Some(level)
    }
}

/// One line to the firmware and its answer, in bounded time whatever state the
/// port is in. Stock firmware never reads its console: once its buffer is
/// full a blocking write would wait for ever, and a close with unsent bytes
/// waits 30s for them.
fn ask(path: &Path, command: &str) -> Answer {
    let Ok(mut port) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(path)
    else {
        return Answer::Silent;
    };
    // `pt35-kbd` holds this lock while it writes the firmware: a line
    // from us landing in its REPL breaks the flash, and so would changing
    // the port's settings under it. Skip, do not wait.
    if !try_lock(&port) {
        return Answer::Busy;
    }
    // Raw and no echo, set every time: `pt35-kbd` leaves its own settings
    // behind. Set here and not by `stty`: the board sends a title on every
    // open, and a new tty echoes it back until the echo is off. The pt35
    // firmware up to 0.4.1 died on that echo. No hang-up on close: every
    // request opens the port anew.
    if !make_raw(&port) {
        return Answer::Silent;
    }
    let mut answer = Answer::Silent;
    if port.write_all(format!("{command}\n").as_bytes()).is_ok() {
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut seen = Vec::new();
        let mut chunk = [0u8; 256];
        while Instant::now() < deadline {
            match port.read(&mut chunk) {
                Ok(n) if n > 0 => {
                    seen.extend_from_slice(&chunk[..n]);
                    if let Some(level) = parse_reply(&String::from_utf8_lossy(&seen)) {
                        answer = Answer::Level(level);
                        break;
                    }
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
    }
    if answer == Answer::Silent {
        discard_unsent(&port);
    }
    answer
}

#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x4;

#[cfg(target_os = "linux")]
const TCOFLUSH: i32 = 1;
#[cfg(not(target_os = "linux"))]
const TCOFLUSH: i32 = 2;

#[cfg(target_os = "linux")]
type TcFlag = u32;
#[cfg(not(target_os = "linux"))]
type TcFlag = u64;

#[cfg(target_os = "linux")]
const HUPCL: TcFlag = 0o2000;
#[cfg(not(target_os = "linux"))]
const HUPCL: TcFlag = 0x4000;

/// `struct termios` up to `c_cflag`, with room for the rest of it.
#[repr(C)]
struct Termios {
    iflag: TcFlag,
    oflag: TcFlag,
    cflag: TcFlag,
    lflag: TcFlag,
    rest: [u8; 64],
}

extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn tcflush(fd: i32, queue: i32) -> i32;
    fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, when: i32, termios: *const Termios) -> i32;
    fn cfmakeraw(termios: *mut Termios);
}

/// `stty raw -echo -hupcl` on an open port.
fn make_raw(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    const TCSANOW: i32 = 0;
    let mut termios = Termios {
        iflag: 0,
        oflag: 0,
        cflag: 0,
        lflag: 0,
        rest: [0; 64],
    };
    // SAFETY: a valid open fd, and a buffer larger than `struct termios`.
    unsafe {
        if tcgetattr(file.as_raw_fd(), &mut termios) != 0 {
            return false;
        }
        cfmakeraw(&mut termios);
        termios.cflag &= !HUPCL;
        tcsetattr(file.as_raw_fd(), TCSANOW, &termios) == 0
    }
}

/// An exclusive, non-blocking flock, released when the file closes.
fn try_lock(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    // SAFETY: a valid open fd and plain integer flags.
    unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) == 0 }
}

/// Drop what the keyboard has not taken, so the close does not wait for it.
fn discard_unsent(file: &std::fs::File) {
    use std::os::fd::AsRawFd;
    // SAFETY: a valid open fd and a plain integer selector.
    unsafe {
        tcflush(file.as_raw_fd(), TCOFLUSH);
    }
}

/// The keyboard's console under /dev/serial/by-id, else the first ACM port.
fn console() -> Option<PathBuf> {
    let named = std::fs::read_dir("/dev/serial/by-id").ok().and_then(|dir| {
        dir.flatten()
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().contains("Pico"))
    });
    named.or_else(|| Some(PathBuf::from("/dev/ttyACM0")).filter(|p| p.exists()))
}

/// The last whole `PT35 B=<n>` line in what came back. The firmware prints
/// debug lines on the same port, and CircuitPython a title-bar escape with no
/// newline the moment the port opens, so the reply is searched for, not read
/// as a line. Its line end is what says the number is all there.
pub fn parse_reply(text: &str) -> Option<u8> {
    text.rmatch_indices("PT35 B=").find_map(|(at, tag)| {
        let after = &text[at + tag.len()..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 || !after[digits..].starts_with(['\r', '\n']) {
            return None;
        }
        after[..digits].parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_is_found_among_debug_lines() {
        assert_eq!(parse_reply("[HID] ready\r\nPT35 B=40\r\n"), Some(40));
        assert_eq!(
            parse_reply("PT35 B=40\nPT35 B=55\n"),
            Some(55),
            "the latest"
        );
        assert_eq!(parse_reply("bl_pwm: 3000\n"), None);
        let opened = "\x1b]0;code.py | 10.0.0\x1b\\PT35 B=92\r\n";
        assert_eq!(parse_reply(opened), Some(92), "after the title escape");
    }

    #[test]
    fn half_a_reply_is_not_one() {
        assert_eq!(parse_reply("PT35 B=4"), None, "the 0 is still on its way");
        assert_eq!(parse_reply("PT35 B=40\r\nPT35 B=5"), Some(40));
        assert_eq!(parse_reply("PT35 B=\r\n"), None);
    }
}
