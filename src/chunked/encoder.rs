use std::pin::Pin;

use async_std::io;
use async_std::io::prelude::*;
use async_std::task::{Context, Poll};
use futures_core::ready;

/// An encoder for chunked encoding.
#[derive(Debug)]
pub(crate) struct ChunkedEncoder<R> {
    reader: R,
    done: bool,
}

impl<R: Read + Unpin> ChunkedEncoder<R> {
    /// Create a new instance.
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            done: false,
        }
    }
}

fn max_bytes_to_read(buf_len: usize) -> usize {
    if buf_len < 6 {
        panic!("buffers of length {} are too small for this implementation. if this is a problem for you, please open an issue", buf_len);
    }
    let max_bytes_of_hex_framing = // the maximum number of bytes that the hex representation of remaining bytes might take
                (((buf_len - 5) as f64).log2() / 4f64).floor() as usize;
    buf_len - 5 - max_bytes_of_hex_framing
}

impl<R: Read + Unpin> Read for ChunkedEncoder<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.done {
            return Poll::Ready(Ok(0));
        }
        let reader = &mut self.reader;

        let max_bytes_to_read = max_bytes_to_read(buf.len());

        let bytes = ready!(Pin::new(reader).poll_read(cx, &mut buf[..max_bytes_to_read]))?;
        if bytes == 0 {
            self.done = true;
        }
        let start = format!("{:X}\r\n", bytes);
        let start_length = start.as_bytes().len();
        let total = bytes + start_length + 2;
        buf.copy_within(..bytes, start_length);
        buf[..start_length].copy_from_slice(start.as_bytes());
        buf[total - 2..total].copy_from_slice(b"\r\n");
        Poll::Ready(Ok(total))
    }
}
