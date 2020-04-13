use std::pin::Pin;

use async_std::io;
use async_std::io::prelude::*;
use async_std::task::{Context, Poll};
use http_types::Response;

const CR: u8 = b'\r';
const LF: u8 = b'\n';

/// An encoder for chunked encoding.
#[derive(Debug)]
pub(crate) struct ChunkedEncoder {
    done: bool,
}

impl ChunkedEncoder {
    /// Create a new instance.
    pub(crate) fn new() -> Self {
        Self { done: false }
    }
    /// Encode an AsyncBufRead using "chunked" framing. This is used for streams
    /// whose length is not known up front.
    ///
    /// # Format
    ///
    /// Each "chunk" uses the following encoding:
    ///
    /// ```txt
    /// 1. {byte length of `data` as hex}\r\n
    /// 2. {data}\r\n
    /// ```
    ///
    /// A chunk stream is finalized by appending the following:
    ///
    /// ```txt
    /// 1. 0\r\n
    /// 2. {trailing header}\r\n (can be repeated)
    /// 3. \r\n
    /// ```
    pub(crate) fn encode(
        &mut self,
        mut res: &mut Response,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut bytes_read = 0;

        // Return early if we know we're done.
        if self.done {
            return Poll::Ready(Ok(0));
        }

        // Get bytes from the underlying stream. If the stream is not ready yet,
        // return the header bytes if we have any.
        let src = match Pin::new(&mut res).poll_fill_buf(cx) {
            Poll::Ready(Ok(n)) => n,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => match bytes_read {
                0 => return Poll::Pending,
                n => return Poll::Ready(Ok(n)),
            },
        };

        // If the stream doesn't have any more bytes left to read we're done.
        if src.len() == 0 {
            // Write out the final empty chunk
            let idx = bytes_read;
            buf[idx] = b'0';
            buf[idx + 1] = CR;
            buf[idx + 2] = LF;

            // Write the final CRLF
            buf[idx + 3] = CR;
            buf[idx + 4] = LF;
            bytes_read += 5;

            log::trace!("done sending bytes");
            self.done = true;
            return Poll::Ready(Ok(bytes_read));
        }

        // Each chunk is prefixed with the length of the data in hex, then a
        // CRLF, then the content, then another CRLF. Calculate how many bytes
        // each part should be.
        let buf_len = buf.len().checked_sub(bytes_read).unwrap_or(0);
        let amt = src.len().min(buf_len);
        // Calculate the max char count encoding the `len_prefix` statement
        // as hex would take. This is done by rounding up `log16(amt + 1)`.
        let hex_len = ((amt + 1) as f64).log(16.0).ceil() as usize;
        let crlf_len = 2 * 2;
        let buf_upper = buf_len.checked_sub(hex_len + crlf_len).unwrap_or(0);
        let amt = amt.min(buf_upper);
        let len_prefix = format!("{:X}", amt).into_bytes();

        // Write our frame header to the buffer.
        let lower = bytes_read;
        let upper = bytes_read + len_prefix.len();
        buf[lower..upper].copy_from_slice(&len_prefix);
        buf[upper] = CR;
        buf[upper + 1] = LF;
        bytes_read += len_prefix.len() + 2;

        // Copy the bytes from our source into the output buffer.
        let lower = bytes_read;
        let upper = bytes_read + amt;
        buf[lower..upper].copy_from_slice(&src[0..amt]);
        Pin::new(&mut res).consume(amt);
        bytes_read += amt;

        // Finalize the chunk with a final CRLF.
        let idx = bytes_read;
        buf[idx] = CR;
        buf[idx + 1] = LF;
        bytes_read += 2;

        // Finally return how many bytes we've written to the buffer.
        log::trace!("sending {} bytes", bytes_read);
        Poll::Ready(Ok(bytes_read))
    }
}
