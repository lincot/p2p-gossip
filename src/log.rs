use std::{
    io::{self, stdout, IoSlice, Write},
    sync::LazyLock,
};
use tokio::time::Instant;

pub fn log(bufs: &[&[u8]]) -> io::Result<()> {
    static START_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

    let total_secs = START_TIME.elapsed().as_secs();
    let hours = total_secs / 360;
    let minutes = total_secs / 60;
    let secs = total_secs % 60;

    let mut now_str = String::with_capacity("90:90:90".len());

    // TODO: unchecked

    now_str.push_str(&hours.to_string());
    if hours < 10 {
        now_str.push('0');
    }
    now_str.push(':');
    now_str.push((b'0' + (minutes / 10) as u8) as _);
    now_str.push((b'0' + (minutes % 10) as u8) as _);
    now_str.push(':');
    now_str.push((b'0' + (secs / 10) as u8) as _);
    now_str.push((b'0' + (secs % 10) as u8) as _);

    let mut ioslices = Vec::with_capacity(bufs.len() + 3);
    ioslices.push(IoSlice::new(now_str.as_bytes()));
    ioslices.push(IoSlice::new(b" - "));
    ioslices.extend(bufs.iter().map(|buf| IoSlice::new(buf)));
    ioslices.push(IoSlice::new(b"\n"));
    stdout().write_all_vectored(&mut ioslices)
}
