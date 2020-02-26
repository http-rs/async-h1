//! Process HTTP connections on the server.

use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self, BufReader};
use async_std::io::{Read, Write};
use async_std::prelude::*;
use async_std::task::{Context, Poll};
use futures_core::ready;
use http_types::headers::{HeaderName, HeaderValue, CONTENT_LENGTH, TRANSFER_ENCODING};
use http_types::{ensure, ensure_eq, format_err};
use http_types::{Body, Method, Request, Response};

use crate::chunked::ChunkedDecoder;
use crate::date::fmt_http_date;
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

const CR: u8 = b'\r';
const LF: u8 = b'\n';

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept<RW, F, Fut>(addr: &str, mut io: RW, endpoint: F) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    // TODO: make configurable
    let timeout_duration = Duration::from_secs(10);
    const MAX_REQUESTS: usize = 200;
    let mut num_requests = 0;

    // Decode a request. This may be the first of many since the connection is Keep-Alive by default.
    let r = io.clone();
    let req = decode(addr, r).await?;

    if let Some(mut req) = req {
        loop {
            match num_requests {
                MAX_REQUESTS => return Ok(()),
                _ => num_requests += 1,
            };

            // TODO: what to do when the endpoint returns Err
            let res = endpoint(req).await?;
            let mut encoder = Encoder::encode(res);
            io::copy(&mut encoder, &mut io).await?;

            // Decode a new request, timing out if this takes longer than the
            // timeout duration.
            req = match timeout(timeout_duration, decode(addr, io.clone())).await {
                Ok(Ok(Some(r))) => r,
                Ok(Ok(None)) | Err(TimeoutError { .. }) => break, /* EOF or timeout */
                Ok(Err(e)) => return Err(e).into(),
            };
            // Loop back with the new request and stream and start again
        }
    }

    Ok(())
}

/// A streaming HTTP encoder.
///
/// This is returned from [`encode`].
#[derive(Debug)]
struct Encoder {
    /// HTTP headers to be sent.
    res: Response,
    /// The state of the encoding process
    state: EncoderState,
}

#[derive(Debug)]
enum EncoderState {
    Start,
    Head {
        data: Vec<u8>,
        head_bytes_read: usize,
    },
    Body {
        body_bytes_read: usize,
        body_len: usize,
    },
    UncomputedChunked,
    ComputedChunked {
        chunk: io::Cursor<Vec<u8>>,
        is_last: bool,
    },
    Done,
}

impl Encoder {
    /// Create a new instance.
    pub(crate) fn encode(res: Response) -> Self {
        Self {
            res,
            state: EncoderState::Start,
        }
    }

    fn encode_head(&self) -> io::Result<Vec<u8>> {
        let mut head: Vec<u8> = vec![];

        let reason = self.res.status().canonical_reason();
        let status = self.res.status();
        std::io::Write::write_fmt(
            &mut head,
            format_args!("HTTP/1.1 {} {}\r\n", status, reason),
        )?;

        // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
        // send all items in chunks.
        if let Some(len) = self.res.len() {
            std::io::Write::write_fmt(&mut head, format_args!("content-length: {}\r\n", len))?;
        } else {
            std::io::Write::write_fmt(&mut head, format_args!("transfer-encoding: chunked\r\n"))?;
        }

        let date = fmt_http_date(std::time::SystemTime::now());
        std::io::Write::write_fmt(&mut head, format_args!("date: {}\r\n", date))?;

        for (header, values) in self.res.iter() {
            for value in values.iter() {
                std::io::Write::write_fmt(&mut head, format_args!("{}: {}\r\n", header, value))?
            }
        }

        std::io::Write::write_fmt(&mut head, format_args!("\r\n"))?;

        Ok(head)
    }
}

impl Read for Encoder {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // we must keep track how many bytes of the head and body we've read
        // in this call of `poll_read`
        let mut bytes_read = 0;
        loop {
            match self.state {
                EncoderState::Start => {
                    // Encode the headers to a buffer, the first time we poll
                    let head = self.encode_head()?;
                    self.state = EncoderState::Head {
                        data: head,
                        head_bytes_read: 0,
                    };
                }
                EncoderState::Head {
                    ref data,
                    mut head_bytes_read,
                } => {
                    // Read from the serialized headers, url and methods.
                    let head_len = data.len();
                    let len = std::cmp::min(head_len - head_bytes_read, buf.len());
                    let range = head_bytes_read..head_bytes_read + len;
                    buf[0..len].copy_from_slice(&data[range]);
                    bytes_read += len;
                    head_bytes_read += len;

                    // If we've read the total length of the head we're done
                    // reading the head and can transition to reading the body
                    if head_bytes_read == head_len {
                        // The response length lets us know if we are encoding
                        // our body in chunks or not
                        self.state = match self.res.len() {
                            Some(body_len) => EncoderState::Body {
                                body_bytes_read: 0,
                                body_len,
                            },
                            None => EncoderState::UncomputedChunked,
                        };
                    } else {
                        // If we haven't read the entire header it means `buf` isn't
                        // big enough. Break out of loop and return from `poll_read`
                        break;
                    }
                }
                EncoderState::Body {
                    mut body_bytes_read,
                    body_len,
                } => {
                    // Double check that we didn't somehow read more bytes than
                    // can fit in our buffer
                    debug_assert!(bytes_read <= buf.len());

                    // ensure we have at least room for 1 more byte in our buffer
                    if bytes_read == buf.len() {
                        break;
                    }

                    // Figure out how many bytes we can read.
                    let upper_bound = (bytes_read + body_len - body_bytes_read).min(buf.len());
                    // Read bytes from body
                    let new_body_bytes_read =
                        ready!(Pin::new(&mut self.res)
                            .poll_read(cx, &mut buf[bytes_read..upper_bound]))?;
                    body_bytes_read += new_body_bytes_read;
                    bytes_read += new_body_bytes_read;

                    // Double check we did not read more body bytes than the total
                    // length of the body
                    debug_assert!(
                        body_bytes_read <= body_len,
                        "Too many bytes read. Expected: {}, read: {}",
                        body_len,
                        body_bytes_read
                    );
                    // If we've read the `len` number of bytes, end
                    if body_len == body_bytes_read {
                        self.state = EncoderState::Done;
                        break;
                    } else {
                        self.state = EncoderState::Body {
                            body_bytes_read,
                            body_len,
                        }
                    }
                }
                EncoderState::UncomputedChunked => {
                    // We can read a maximum of the buffer's total size
                    // minus what we've already filled the buffer with
                    let buffer_remaining = buf.len() - bytes_read;

                    // ensure we have at least room for 1 byte in our buffer
                    if buffer_remaining == 0 {
                        break;
                    }
                    // we must allocate a separate buffer for the chunk data
                    // since we first need to know its length before writing
                    // it into the actual buffer
                    let mut chunk_buf = vec![0; buffer_remaining];
                    // Read bytes from body reader
                    let chunk_length =
                        ready!(Pin::new(&mut self.res).poll_read(cx, &mut chunk_buf))?;

                    // serialize chunk length as hex
                    let chunk_length_string = format!("{:X}", chunk_length);
                    let chunk_length_bytes = chunk_length_string.as_bytes();
                    let chunk_length_bytes_len = chunk_length_bytes.len();
                    const CRLF_LENGTH: usize = 2;

                    // calculate the total size of the chunk including serialized
                    // length and the CRLF padding
                    let total_chunk_size = bytes_read
                        + chunk_length_bytes_len
                        + CRLF_LENGTH
                        + chunk_length
                        + CRLF_LENGTH;

                    // See if we can write the chunk out in one go
                    if total_chunk_size < buffer_remaining {
                        // Write the chunk length into the buffer
                        buf[bytes_read..bytes_read + chunk_length_bytes_len]
                            .copy_from_slice(chunk_length_bytes);
                        bytes_read += chunk_length_bytes_len;

                        // follow chunk length with CRLF
                        buf[bytes_read] = CR;
                        buf[bytes_read + 1] = LF;
                        bytes_read += 2;

                        // copy chunk into buf
                        buf[bytes_read..bytes_read + chunk_length]
                            .copy_from_slice(&chunk_buf[..chunk_length]);
                        bytes_read += chunk_length;

                        // follow chunk with CRLF
                        buf[bytes_read] = CR;
                        buf[bytes_read + 1] = LF;
                        bytes_read += 2;

                        if chunk_length == 0 {
                            self.state = EncoderState::Done;
                            break;
                        }
                    } else {
                        let mut chunk = vec![0; total_chunk_size];
                        let mut bytes_written = 0;
                        // Write the chunk length into the buffer
                        chunk[0..chunk_length_bytes_len].copy_from_slice(chunk_length_bytes);
                        bytes_written += chunk_length_bytes_len;

                        // follow chunk length with CRLF
                        chunk[bytes_written] = CR;
                        chunk[bytes_written + 1] = LF;
                        bytes_written += 2;

                        // copy chunk into buf
                        chunk[bytes_written..bytes_written + chunk_length]
                            .copy_from_slice(&chunk_buf[..chunk_length]);
                        bytes_written += chunk_length;

                        // follow chunk with CRLF
                        chunk[bytes_written] = CR;
                        chunk[bytes_written + 1] = LF;
                        bytes_read += 2;
                        self.state = EncoderState::ComputedChunked {
                            chunk: io::Cursor::new(chunk),
                            is_last: chunk_length == 0,
                        };
                    }
                }
                EncoderState::ComputedChunked {
                    ref mut chunk,
                    is_last,
                } => {
                    bytes_read += ready!(Pin::new(chunk).poll_read(cx, &mut buf))?;
                    if bytes_read == 0 {
                        self.state = match is_last {
                            true => EncoderState::Done,
                            false => EncoderState::UncomputedChunked,
                        }
                    }
                    break;
                }
                EncoderState::Done => break,
            }
        }

        Poll::Ready(Ok(bytes_read as usize))
    }
}

/// The number returned from httparse when the request is HTTP 1.1
const HTTP_1_1_VERSION: u8 = 1;

/// Decode an HTTP request on the server.
async fn decode<R>(addr: &str, reader: R) -> http_types::Result<Option<Request>>
where
    R: Read + Unpin + Send + Sync + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut httparse_req = httparse::Request::new(&mut headers);

    // Keep reading bytes from the stream until we hit the end of the stream.
    loop {
        let bytes_read = reader.read_until(LF, &mut buf).await?;
        // No more bytes are yielded from the stream.
        if bytes_read == 0 {
            return Ok(None);
        }

        // Prevent CWE-400 DDOS with large HTTP Headers.
        ensure!(
            buf.len() < MAX_HEAD_LENGTH,
            "Head byte length limit exceeded"
        );

        // We've hit the end delimiter of the stream.
        let idx = buf.len() - 1;
        if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
            break;
        }
    }

    // Convert our header buf into an httparse instance, and validate.
    let status = httparse_req.parse(&buf)?;

    ensure!(!status.is_partial(), "Malformed HTTP head");

    // Convert httparse headers + body into a `http_types::Request` type.
    let method = httparse_req.method;
    let method = method.ok_or_else(|| format_err!("No method found"))?;

    let uri = httparse_req.path;
    let uri = uri.ok_or_else(|| format_err!("No uri found"))?;
    let uri = url::Url::parse(&format!("{}{}", addr, uri))?;

    let version = httparse_req.version;
    let version = version.ok_or_else(|| format_err!("No version found"))?;
    ensure_eq!(version, HTTP_1_1_VERSION, "Unsupported HTTP version");

    let mut req = Request::new(Method::from_str(method)?, uri);
    for header in httparse_req.headers.iter() {
        let name = HeaderName::from_str(header.name)?;
        let value = HeaderValue::from_str(std::str::from_utf8(header.value)?)?;
        req.insert_header(name, value)?;
    }

    let content_length = req.header(&CONTENT_LENGTH);
    let transfer_encoding = req.header(&TRANSFER_ENCODING);

    http_types::ensure!(
        content_length.is_none() || transfer_encoding.is_none(),
        "Unexpected Content-Length header"
    );

    // Check for Transfer-Encoding
    if let Some(encoding) = transfer_encoding {
        if !encoding.is_empty() && encoding.last().unwrap().as_str() == "chunked" {
            let trailer_sender = req.send_trailers();
            let reader = BufReader::new(ChunkedDecoder::new(reader, trailer_sender));
            req.set_body(Body::from_reader(reader, None));
            return Ok(Some(req));
        }
        // Fall through to Content-Length
    }

    // Check for Content-Length.
    if let Some(len) = content_length {
        let len = len.last().unwrap().as_str().parse::<usize>()?;
        req.set_body(Body::from_reader(reader.take(len as u64), Some(len)));
    }

    Ok(Some(req))
}
