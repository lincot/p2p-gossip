use std::{
    io::{stdout, Write},
    sync::OnceLock,
};
use tokio::time::Instant;

/// Prints `bufs` to stdout, formatted with the time
/// elapsed since the program was started.
///
/// # Examples
///
/// ```
/// // prints "00:00:05 - onetwo\n"
/// log(&[b"one", b"two"]);
/// ```
pub fn log(bufs: &[&[u8]]) {
    static START_TIME: OnceLock<Instant> = OnceLock::new();

    let time = format_duration(START_TIME.get_or_init(Instant::now).elapsed().as_secs());

    let mut out = stdout();
    out.write_all(time.as_bytes()).unwrap();
    out.write_all(b" - ").unwrap();
    for buf in bufs {
        out.write_all(buf).unwrap();
    }
    out.write_all(b"\n").unwrap();
}

/// Formats a duration `seconds` in HH:MM:SS format.
///
/// # Examples
///
/// ```
/// assert_eq!(format_duration(5 * 60 * 60 + 12 * 60 + 7), "05:12:07");
/// ```
fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;

    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(333 * 60 * 60 + 22 * 60 + 1), "333:22:01");
        assert_eq!(format_duration(5 * 60 * 60 + 12 * 60 + 7), "05:12:07");
        assert_eq!(format_duration(0), "00:00:00");
        assert_eq!(format_duration(67), "00:01:07");
    }
}
