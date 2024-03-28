use digital::{MaxLenBase10, WriteNumUnchecked};
use std::{
    io::{self, stdout, IoSlice, Write},
    sync::LazyLock,
};
use tokio::time::Instant;
use unchecked_core::PushUnchecked;

pub fn log(bufs: &[&[u8]]) -> io::Result<()> {
    static START_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

    let time = format_time(START_TIME.elapsed().as_secs());

    let mut ioslices = Vec::with_capacity(bufs.len() + 3);
    // SAFETY: it's initialized with sufficient capacity
    unsafe {
        ioslices.push_unchecked(IoSlice::new(time.as_bytes()));
        ioslices.push_unchecked(IoSlice::new(b" - "));
        for buf in bufs {
            ioslices.push_unchecked(IoSlice::new(buf));
        }
        ioslices.push_unchecked(IoSlice::new(b"\n"));
    }

    stdout().write_all_vectored(&mut ioslices)
}

fn format_time(total_secs: u64) -> heapless::String<{ u64::MAX_LEN_BASE10 + ":00:00".len() }> {
    let mut res = heapless::Vec::new();

    let hours = total_secs / 3600;
    let minutes = total_secs / 60;
    let secs = total_secs % 60;

    // SAFETY: buffer length specified in output type is sufficient,
    // all written characters are ASCII
    unsafe {
        if hours < 10 {
            res.push_unchecked(b'0');
        }
        res.write_num_unchecked(hours, 10, false, false);
        res.push_unchecked(b':');
        res.push_unchecked((b'0' + (minutes / 10) as u8) as _);
        res.push_unchecked((b'0' + (minutes % 10) as u8) as _);
        res.push_unchecked(b':');
        res.push_unchecked((b'0' + (secs / 10) as u8) as _);
        res.push_unchecked((b'0' + (secs % 10) as u8) as _);

        heapless::String::from_utf8_unchecked(res)
    }
}
