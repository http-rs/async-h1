//! Process HTTP connections on the server.

use std::pin::Pin;

use async_std::io;
use async_std::io::prelude::*;
use async_std::task::{Context, Poll};
use http_types::Response;

use crate::chunked::ChunkedEncoder;
use crate::date::fmt_http_date;

/// A streaming HTTP encoder.
///
/// This is returned from [`encode`].
#[derive(Debug)]
pub(crate) struct Encoder {
    /// HTTP headers to be sent.
    res: Response,
    /// The state of the encoding process
    state: State,
    /// Track bytes read in a call to poll_read.
    bytes_read: usize,
    /// The data we're writing as part of the head section.
    head: Vec<u8>,
    /// The amount of bytes read from the head section.
    head_bytes_read: usize,
    /// The total length of the body.
    /// This is only used in the known-length body encoder.
    body_len: usize,
    /// The amount of bytes read from the body.
    /// This is only used in the known-length body encoder.
    body_bytes_read: usize,
    /// An encoder for chunked encoding.
    chunked: ChunkedEncoder,
}

#[derive(Debug)]
enum State {
    Init,
    ComputeHead,
    EncodeHead,
    FixedBody,
    ChunkedBody,
    End,
}

impl Encoder {
    /// Create a new instance of Encoder.
    pub(crate) fn new(res: Response) -> Self {
        Self {
            res,
            state: State::Init,
            bytes_read: 0,
            head: vec![],
            head_bytes_read: 0,
            body_len: 0,
            body_bytes_read: 0,
            chunked: ChunkedEncoder::new(),
        }
    }

    pub(crate) fn encode(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes_read = 0;
        let res = match self.state {
            State::Init => self.init(cx, buf),
            State::ComputeHead => self.compute_head(cx, buf),
            State::EncodeHead => self.encode_head(cx, buf),
            State::FixedBody => self.encode_fixed_body(cx, buf),
            State::ChunkedBody => self.encode_chunked_body(cx, buf),
            State::End => Poll::Ready(Ok(self.bytes_read)),
        };
        log::trace!("ServerEncoder {} bytes written", self.bytes_read);
        res
    }

    /// Switch the internal state to a new state.
    fn set_state(&mut self, state: State) {
        use State::*;
        log::trace!("Server Encoder state: {:?} -> {:?}", self.state, state);

        #[cfg(debug_assertions)]
        match self.state {
            Init => assert!(matches!(state, ComputeHead)),
            ComputeHead => assert!(matches!(state, EncodeHead)),
            EncodeHead => assert!(matches!(state, ChunkedBody | FixedBody)),
            FixedBody => assert!(matches!(state, End)),
            ChunkedBody => assert!(matches!(state, End)),
            End => panic!("No state transitions allowed after the stream has ended"),
        }

        self.state = state;
    }

    /// Initialize to the first state.
    fn init(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.set_state(State::ComputeHead);
        self.compute_head(cx, buf)
    }

    /// Encode the headers to a buffer, the first time we poll.
    fn compute_head(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let reason = self.res.status().canonical_reason();
        let status = self.res.status();
        std::io::Write::write_fmt(
            &mut self.head,
            format_args!("HTTP/1.1 {} {}\r\n", status, reason),
        )?;

        // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
        // send all items in chunks.
        if let Some(len) = self.res.len() {
            std::io::Write::write_fmt(&mut self.head, format_args!("content-length: {}\r\n", len))?;
        } else {
            std::io::Write::write_fmt(
                &mut self.head,
                format_args!("transfer-encoding: chunked\r\n"),
            )?;
        }

        let date = fmt_http_date(std::time::SystemTime::now());
        std::io::Write::write_fmt(&mut self.head, format_args!("date: {}\r\n", date))?;

        for (header, values) in self.res.iter() {
            for value in values.iter() {
                std::io::Write::write_fmt(
                    &mut self.head,
                    format_args!("{}: {}\r\n", header, value),
                )?
            }
        }

        std::io::Write::write_fmt(&mut self.head, format_args!("\r\n"))?;

        self.set_state(State::EncodeHead);
        self.encode_head(cx, buf)
    }

    /// Encode the status code + headers.
    fn encode_head(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        // Read from the serialized headers, url and methods.
        let head_len = self.head.len();
        let len = std::cmp::min(head_len - self.head_bytes_read, buf.len());
        let range = self.head_bytes_read..self.head_bytes_read + len;
        buf[0..len].copy_from_slice(&self.head[range]);
        self.bytes_read += len;
        self.head_bytes_read += len;

        // If we've read the total length of the head we're done
        // reading the head and can transition to reading the body
        if self.head_bytes_read == head_len {
            // The response length lets us know if we are encoding
            // our body in chunks or not
            match self.res.len() {
                Some(body_len) => {
                    self.body_len = body_len;
                    self.state = State::FixedBody;
                    log::trace!("Server response encoding: fixed length body");
                    return self.encode_fixed_body(cx, buf);
                }
                None => {
                    self.state = State::ChunkedBody;
                    log::trace!("Server response encoding: chunked body");
                    return self.encode_chunked_body(cx, buf);
                }
            };
        } else {
            // If we haven't read the entire header it means `buf` isn't
            // big enough. Break out of loop and return from `poll_read`
            return Poll::Ready(Ok(self.bytes_read));
        }
    }

    /// Encode the body with a known length.
    fn encode_fixed_body(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Double check that we didn't somehow read more bytes than
        // can fit in our buffer
        debug_assert!(self.bytes_read <= buf.len());

        // ensure we have at least room for 1 more byte in our buffer
        if self.bytes_read == buf.len() {
            return Poll::Ready(Ok(self.bytes_read));
        }

        // Figure out how many bytes we can read.
        let upper_bound = (self.bytes_read + self.body_len - self.body_bytes_read).min(buf.len());
        // Read bytes from body
        let range = self.bytes_read..upper_bound;
        let inner_poll_result = Pin::new(&mut self.res).poll_read(cx, &mut buf[range]);
        let new_body_bytes_read = match inner_poll_result {
            Poll::Ready(Ok(n)) => n,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => match self.bytes_read {
                0 => return Poll::Pending,
                n => return Poll::Ready(Ok(n)),
            },
        };
        self.body_bytes_read += new_body_bytes_read;
        self.bytes_read += new_body_bytes_read;

        // Double check we did not read more body bytes than the total
        // length of the body
        debug_assert!(
            self.body_bytes_read <= self.body_len,
            "Too many bytes read. Expected: {}, read: {}",
            self.body_len,
            self.body_bytes_read
        );

        if self.body_len == self.body_bytes_read {
            // If we've read the `len` number of bytes, end
            self.set_state(State::End);
            return Poll::Ready(Ok(self.bytes_read));
        } else if new_body_bytes_read == 0 {
            // If we've reached unexpected EOF, end anyway
            // TODO: do something?
            self.set_state(State::End);
            return Poll::Ready(Ok(self.bytes_read));
        } else {
            self.encode_fixed_body(cx, buf)
        }
    }

    /// Encode an AsyncBufRead using "chunked" framing. This is used for streams
    /// whose length is not known up front.
    fn encode_chunked_body(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let buf = &mut buf[self.bytes_read..];
        match self.chunked.encode(&mut self.res, cx, buf) {
            Poll::Ready(Ok(read)) => {
                self.bytes_read += read;
                if self.bytes_read == 0 {
                    self.set_state(State::End);
                }
                Poll::Ready(Ok(self.bytes_read))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                if self.bytes_read > 0 {
                    return Poll::Ready(Ok(self.bytes_read));
                }
                Poll::Pending
            }
        }
    }
}

impl Read for Encoder {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.encode(cx, buf)
    }
}
