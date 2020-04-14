use std::fmt::{self, Debug};
use std::pin::Pin;

use async_std::future::Future;
use async_std::io;
use async_std::io::prelude::*;
use async_std::task::{Context, Poll};
use http_types::{Response, Trailers};

const CR: u8 = b'\r';
const LF: u8 = b'\n';

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a + Send>>;

/// The encoder state.
#[derive(Debug)]
enum State {
    /// Streaming out chunks.
    Streaming,
    /// Receiving trailers from a channel.
    ReceiveTrailers,
    /// Streaming out trailers.
    EncodeTrailers,
    /// Writing the final CRLF
    EndOfStream,
    /// The stream has finished.
    Done,
}

/// An encoder for chunked encoding.
// #[derive(Debug)]
pub(crate) struct ChunkedEncoder {
    /// How many bytes we've written to the buffer so far.
    bytes_written: usize,
    /// The internal encoder state.
    state: State,
    /// Holds the state of the `Receiver` future.
    recv_fut: Option<BoxFuture<'static, Option<http_types::Result<Trailers>>>>,
}

impl ChunkedEncoder {
    /// Create a new instance.
    pub(crate) fn new() -> Self {
        Self {
            state: State::Streaming,
            bytes_written: 0,
            recv_fut: None,
        }
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
        res: &mut Response,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes_written = 0;
        match self.state {
            State::Streaming => self.encode_stream(res, cx, buf),
            State::ReceiveTrailers => self.encode_trailers(res, cx, buf),
            State::EncodeTrailers => self.encode_trailers(res, cx, buf),
            State::EndOfStream => self.encode_eos(cx, buf),
            State::Done => Poll::Ready(Ok(0)),
        }
    }

    /// Stream out data using chunked encoding.
    fn encode_stream(
        &mut self,
        mut res: &mut Response,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Get bytes from the underlying stream. If the stream is not ready yet,
        // return the header bytes if we have any.
        let src = match Pin::new(&mut res).poll_fill_buf(cx) {
            Poll::Ready(Ok(n)) => n,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => match self.bytes_written {
                0 => return Poll::Pending,
                n => return Poll::Ready(Ok(n)),
            },
        };

        // If the stream doesn't have any more bytes left to read we're done.
        if src.len() == 0 {
            // Write out the final empty chunk
            let idx = self.bytes_written;
            buf[idx] = b'0';
            buf[idx + 1] = CR;
            buf[idx + 2] = LF;
            self.bytes_written += 3;

            self.state = State::ReceiveTrailers;
            return self.receive_trailers(res, cx, buf);
        }

        // Each chunk is prefixed with the length of the data in hex, then a
        // CRLF, then the content, then another CRLF. Calculate how many bytes
        // each part should be.
        let buf_len = buf.len().checked_sub(self.bytes_written).unwrap_or(0);
        let amt = src.len().min(buf_len);
        // Calculate the max char count encoding the `len_prefix` statement
        // as hex would take. This is done by rounding up `log16(amt + 1)`.
        let hex_len = ((amt + 1) as f64).log(16.0).ceil() as usize;
        let crlf_len = 2 * 2;
        let buf_upper = buf_len.checked_sub(hex_len + crlf_len).unwrap_or(0);
        let amt = amt.min(buf_upper);
        let len_prefix = format!("{:X}", amt).into_bytes();

        // Write our frame header to the buffer.
        let lower = self.bytes_written;
        let upper = self.bytes_written + len_prefix.len();
        buf[lower..upper].copy_from_slice(&len_prefix);
        buf[upper] = CR;
        buf[upper + 1] = LF;
        self.bytes_written += len_prefix.len() + 2;

        // Copy the bytes from our source into the output buffer.
        let lower = self.bytes_written;
        let upper = self.bytes_written + amt;
        buf[lower..upper].copy_from_slice(&src[0..amt]);
        Pin::new(&mut res).consume(amt);
        self.bytes_written += amt;

        // Finalize the chunk with a final CRLF.
        let idx = self.bytes_written;
        buf[idx] = CR;
        buf[idx + 1] = LF;
        self.bytes_written += 2;

        // Finally return how many bytes we've written to the buffer.
        log::trace!("sending {} bytes", self.bytes_written);
        Poll::Ready(Ok(self.bytes_written))
    }

    /// Receive trailers sent to the response, and store them in an internal
    /// buffer.
    fn receive_trailers(
        &mut self,
        res: &mut Response,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.recv_fut.is_none() {
            let fut = res.recv_trailers();
            self.recv_fut = Some(Box::pin(async move { fut.await }));
        }

        match Pin::new(&mut self.recv_fut.as_mut().unwrap()).poll(cx) {
            // No trailers were received, finish stream.
            Poll::Ready(None) => {
                self.state = State::EndOfStream;
                self.encode_eos(cx, buf)
            }
            // We've received trailers, proceed to encode them.
            Poll::Ready(Some(trailers)) => {
                let trailers =
                    trailers.expect("Trailers should never return errors. See http-types#96");

                self.state = State::EncodeTrailers;
                self.encode_trailers(res, cx, buf)
            }
            // We're waiting on trailers to be received, return.
            Poll::Pending => match self.bytes_written {
                0 => Poll::Pending,
                n => Poll::Ready(Ok(n)),
            },
        }
    }

    /// Send trailers to the buffer.
    fn encode_trailers(
        &mut self,
        _res: &mut Response,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // TODO: actually encode trailers here.
        self.state = State::EndOfStream;
        self.encode_eos(cx, buf)
    }

    /// Encode the end of the stream.
    fn encode_eos(&mut self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let idx = self.bytes_written;
        // Write the final CRLF
        buf[idx] = CR;
        buf[idx + 1] = LF;
        self.bytes_written += 2;

        log::trace!("finished encoding chunked stream");
        self.state = State::Done;
        return Poll::Ready(Ok(self.bytes_written));
    }
}

impl Debug for ChunkedEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!();
    }
}
