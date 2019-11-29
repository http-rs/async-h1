//! Process HTTP connections on the server.

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self, BufRead, BufReader};
use async_std::io::{Read, Write};
use async_std::prelude::*;
use async_std::task::{Context, Poll};
use futures_core::ready;
use http_types::{Method, Request, Response};
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use std::pin::Pin;

use crate::date::fmt_http_date;
use crate::{Exception, MAX_HEADERS};

/// Parse an incoming HTTP connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept<R, W, F, Fut>(
    addr: &str,
    reader: R,
    mut writer: W,
    endpoint: F,
) -> Result<(), Exception>
where
    R: Read + Unpin + Send + 'static,
    W: Write + Unpin,
    F: Fn(&mut Request) -> Fut,
    Fut: Future<Output = Result<Response, Exception>>,
{
    // TODO: make configurable
    let timeout_duration = Duration::from_secs(10);
    const MAX_REQUESTS: usize = 200;
    let mut num_requests = 0;

    // Decode a request. This may be the first of many since
    // the connection is Keep-Alive by default
    let decoded = decode(addr, reader).await?;
    // Decode returns one of three things;
    // * A request with its body reader set to the underlying TCP stream
    // * A request with an empty body AND the underlying stream
    // * No request (because of the stream closed) and no underlying stream
    if let Some(mut decoded) = decoded {
        loop {
            num_requests += 1;
            if num_requests > MAX_REQUESTS {
                // We've exceeded the max number of requests per connection
                return Ok(());
            }

            // Pass the request to the user defined request handler endpoint.
            // Encode the response we get back.
            // TODO: what to do when the endpoint returns Err
            let res = endpoint(decoded.mut_request()).await?;
            let mut encoder = Encoder::encode(res);

            // If we have reference to the stream, unwrap it. Otherwise,
            // get the underlying stream from the request
            let to_decode = decoded.into_reader();

            // Copy the response into the writer
            // TODO: don't double wrap BufReaders, but instead write a version of
            // io::copy that expects a BufReader.
            io::copy(&mut encoder, &mut writer).await?;

            // Decode a new request, timing out if this takes longer than the
            // timeout duration.
            decoded = match timeout(timeout_duration, decode(addr, to_decode)).await {
                Ok(Ok(Some(r))) => r,
                Ok(Ok(None)) | Err(TimeoutError { .. }) => break, /* EOF or timeout */
                Ok(Err(e)) => return Err(e),
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
            std::io::Write::write_fmt(&mut head, format_args!("Content-Length: {}\r\n", len))?;
        } else {
            std::io::Write::write_fmt(&mut head, format_args!("Transfer-Encoding: chunked\r\n"))?;
        }

        let date = fmt_http_date(std::time::SystemTime::now());
        std::io::Write::write_fmt(&mut head, format_args!("Date: {}\r\n", date))?;

        for (header, value) in self.res.headers().iter() {
            std::io::Write::write_fmt(
                &mut head,
                format_args!("{}: {}\r\n", header.as_str(), value),
            )?
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
                    self.state = if body_len == body_bytes_read {
                        EncoderState::Done
                    } else {
                        EncoderState::Body {
                            body_bytes_read,
                            body_len,
                        }
                    };
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
                        buf[bytes_read] = b'\r';
                        buf[bytes_read + 1] = b'\n';
                        bytes_read += 2;

                        // copy chunk into buf
                        buf[bytes_read..bytes_read + chunk_length]
                            .copy_from_slice(&chunk_buf[..chunk_length]);
                        bytes_read += chunk_length;

                        // follow chunk with CRLF
                        buf[bytes_read] = b'\r';
                        buf[bytes_read + 1] = b'\n';
                        bytes_read += 2;

                        if chunk_length == 0 {
                            self.state = EncoderState::Done;
                        }
                    } else {
                        let mut chunk = vec![0; total_chunk_size];
                        let mut bytes_written = 0;
                        // Write the chunk length into the buffer
                        chunk[0..chunk_length_bytes_len].copy_from_slice(chunk_length_bytes);
                        bytes_written += chunk_length_bytes_len;

                        // follow chunk length with CRLF
                        chunk[bytes_written] = b'\r';
                        chunk[bytes_written + 1] = b'\n';
                        bytes_written += 2;

                        // copy chunk into buf
                        chunk[bytes_written..bytes_written + chunk_length]
                            .copy_from_slice(&chunk_buf[..chunk_length]);
                        bytes_written += chunk_length;

                        // follow chunk with CRLF
                        chunk[bytes_written] = b'\r';
                        chunk[bytes_written + 1] = b'\n';
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
async fn decode<R>(addr: &str, reader: R) -> Result<Option<DecodedRequest>, Exception>
where
    R: Read + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut httparse_req = httparse::Request::new(&mut headers);

    // Keep reading bytes from the stream until we hit the end of the stream.
    loop {
        let bytes_read = reader.read_until(b'\n', &mut buf).await?;
        // No more bytes are yielded from the stream.
        if bytes_read == 0 {
            return Ok(None);
        }

        // We've hit the end delimiter of the stream.
        let idx = buf.len() - 1;
        if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
            break;
        }
    }

    // Convert our header buf into an httparse instance, and validate.
    let status = httparse_req.parse(&buf)?;
    if status.is_partial() {
        return Err("Malformed HTTP head".into());
    }

    // Convert httparse headers + body into a `http::Request` type.
    let method = httparse_req.method.ok_or_else(|| "No method found")?;
    let uri = httparse_req.path.ok_or_else(|| "No uri found")?;
    let uri = url::Url::parse(&format!("{}{}", addr, uri))?;
    let version = httparse_req.version.ok_or_else(|| "No version found")?;
    if version != HTTP_1_1_VERSION {
        return Err("Unsupported HTTP version".into());
    }
    let mut req = Request::new(Method::from_str(method)?, uri);
    for header in httparse_req.headers.iter() {
        req = req.set_header(header.name, std::str::from_utf8(header.value)?)?;
    }

    // Check for content-length, that determines determines whether we can parse
    // it with a known length, or need to use chunked encoding.
    let len = match req.header("Content-Length") {
        Some(len) => len.parse::<usize>()?,
        None => return Ok(Some(DecodedRequest::WithoutBody(req, Box::new(reader)))),
    };
    req = req.set_body_reader(reader).set_len(len);
    Ok(Some(DecodedRequest::WithBody(req)))
}

/// A decoded request
enum DecodedRequest {
    /// The TCP connection is inside the request already, so the lifetimes match up.
    WithBody(Request),
    /// The TCP connection is *not* inside the request body, so we need to pass
    /// it along with it to make the lifetimes match up.
    WithoutBody(Request, Box<dyn BufRead + Unpin + Send + 'static>),
}

impl fmt::Debug for DecodedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodedRequest::WithBody(_) => write!(f, "WithBody"),
            DecodedRequest::WithoutBody(_, _) => write!(f, "WithoutBody"),
        }
    }
}

impl DecodedRequest {
    /// Get a mutable reference to the request
    fn mut_request(&mut self) -> &mut Request {
        match self {
            DecodedRequest::WithBody(r) => r,
            DecodedRequest::WithoutBody(r, _) => r,
        }
    }

    /// Consume self and get access to the underlying reader
    ///
    /// When the request has a body, the underlying reader is the body.
    /// When it does not, the underlying body has been passed alongside the request.
    fn into_reader(self) -> Box<dyn BufRead + Unpin + Send + 'static> {
        match self {
            DecodedRequest::WithBody(r) => r.into_body_reader(),
            DecodedRequest::WithoutBody(_, s) => s,
        }
    }
}
