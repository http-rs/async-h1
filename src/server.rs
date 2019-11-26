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

use crate::{Exception, MAX_HEADERS};

pub async fn connect<R, W, F, Fut>(
    addr: &str,
    reader: R,
    mut writer: W,
    callback: F,
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

            // Pass the request to the user defined request handler callback.
            // Encode the response we get back.
            // TODO: what to do when the callback returns Err
            let mut res = encode(callback(decoded.mut_request()).await?).await?;

            // If we have reference to the stream, unwrap it. Otherwise,
            // get the underlying stream from the request
            let to_decode = decoded.into_reader();

            // Copy the response into the writer
            io::copy(&mut res, &mut writer).await?;

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
pub struct Encoder {
    /// Keep track how far we've indexed into the headers + body.
    cursor: usize,
    /// HTTP headers to be sent.
    headers: Vec<u8>,
    /// Check whether we're done sending headers.
    headers_done: bool,
    /// Response containing the HTTP body to be sent.
    response: Response,
    /// Check whether we're done with the body.
    body_done: bool,
    /// Keep track of how many bytes have been read from the body stream.
    body_bytes_read: usize,
}

impl Encoder {
    /// Create a new instance.
    pub(crate) fn new(headers: Vec<u8>, response: Response) -> Self {
        Self {
            response,
            headers,
            cursor: 0,
            headers_done: false,
            body_done: false,
            body_bytes_read: 0,
        }
    }
}

impl Read for Encoder {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Send the headers. As long as the headers aren't fully sent yet we
        // keep sending more of the headers.
        let mut bytes_read = 0;
        if !self.headers_done {
            let len = std::cmp::min(self.headers.len() - self.cursor, buf.len());
            let range = self.cursor..self.cursor + len;
            buf[0..len].copy_from_slice(&mut self.headers[range]);
            self.cursor += len;
            if self.cursor == self.headers.len() {
                self.headers_done = true;
            }
            bytes_read += len;
        }

        if !self.body_done {
            let n = ready!(Pin::new(&mut self.response).poll_read(cx, &mut buf[bytes_read..]))?;
            bytes_read += n;
            self.body_bytes_read += n;
            if bytes_read == 0 {
                self.body_done = true;
            }
        }

        Poll::Ready(Ok(bytes_read as usize))
    }
}

/// Encode an HTTP request on the server.
// TODO: return a reader in the response
pub async fn encode(res: Response) -> io::Result<Encoder> {
    let mut buf: Vec<u8> = vec![];

    let reason = res.status().canonical_reason();
    let status = res.status();
    std::io::Write::write_fmt(&mut buf, format_args!("HTTP/1.1 {} {}\r\n", status, reason))?;

    // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
    // send all items in chunks.
    if let Some(len) = res.len() {
        std::io::Write::write_fmt(&mut buf, format_args!("Content-Length: {}\r\n", len))?;
    } else {
        std::io::Write::write_fmt(&mut buf, format_args!("Transfer-Encoding: chunked\r\n"))?;
        panic!("chunked encoding is not implemented yet");
        // See: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Transfer-Encoding
        //      https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Trailer
    }

    for (header, value) in res.headers().iter() {
        std::io::Write::write_fmt(&mut buf, format_args!("{}: {}\r\n", header.as_str(), value))?
    }

    std::io::Write::write_fmt(&mut buf, format_args!("\r\n"))?;
    Ok(Encoder::new(buf, res))
}

/// The number returned from httparse when the request is HTTP 1.1
const HTTP_1_1_VERSION: u8 = 1;

/// Decode an HTTP request on the server.
pub async fn decode<R>(addr: &str, reader: R) -> Result<Option<DecodedRequest>, Exception>
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
        dbg!(String::from_utf8(buf).unwrap());
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
pub enum DecodedRequest {
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
