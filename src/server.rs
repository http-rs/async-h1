//! Process HTTP connections on the server.

use async_std::io::{self, BufRead, BufReader};
use async_std::task::{Context, Poll};
use futures_io::AsyncRead;
use http::{Request, Response, Version};

use std::pin::Pin;

use crate::{Body, Exception, MAX_HEADERS};

/// A streaming HTTP encoder.
#[derive(Debug)]
pub struct Encoder<R> {
    headers: Vec<u8>,
    headers_done: bool,
    cursor: usize,
    body: Body<R>,
}

impl<R: AsyncRead> Encoder<R> {
    /// Create a new instance.
    pub(crate) fn new(headers: Vec<u8>, body: Body<R>) -> Self {
        Self {
            body,
            headers,
            cursor: 0,
            headers_done: false,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Encoder<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.headers_done {
            return Poll::Ready(Ok(0));
        }
        let len = std::cmp::min(self.headers.len() - self.cursor, buf.len());
        let range = self.cursor..self.cursor + len;
        buf[0..len].copy_from_slice(&mut self.headers[range]);
        self.cursor += len;
        if self.cursor >= self.headers.len() {
            self.headers_done = true;
        }
        Poll::Ready(Ok(len as usize))
    }
}

/// Encode an HTTP request on the server.
// TODO: return a reader in the response
pub async fn encode<R>(res: Response<Body<R>>) -> Result<Encoder<R>, std::io::Error> where R: AsyncRead {
    // TODO: reuse allocations.
    let mut head = Vec::new();
    let status = res.status();
    head.extend_from_slice(b"HTTP/1.1 ");
    head.extend_from_slice(status.as_str().as_bytes());
    head.extend_from_slice(b" ");
    head.extend_from_slice(status.canonical_reason().unwrap().as_bytes());
    head.extend_from_slice(b"\r\n");

    for (header, value) in res.headers() {
        head.extend_from_slice(header.as_str().as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value.to_str().expect("Invalid header value").as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");

    // TODO: serialize body
    Ok(Encoder::new(head, res.into_body()))
}

/// Decode an HTTP request on the server.
pub async fn decode<R>(reader: R) -> Result<Request<Body<BufReader<R>>>, Exception>
    where R: AsyncRead + Unpin + Send,
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
            break;
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
    let mut req = Request::builder();
    for header in httparse_req.headers.iter() {
        req.header(header.name, header.value);
    }
    if let Some(method) = httparse_req.method {
        req.method(method);
    }
    if let Some(path) = httparse_req.path {
        req.uri(path);
    }
    if let Some(version) = httparse_req.version {
        req.version(match version {
            1 => Version::HTTP_11,
            _ => return Err("Unsupported HTTP version".into()),
        });
    }

    // Process the body if `Content-Length` was passed.
    let body = match httparse_req.headers.iter().find(|h| h.name == "Content-Length") {
        Some(_header) => Body::new(reader), // TODO: use the header value
        None => Body::empty(),
    };

    // Return the request.
    Ok(req.body(body)?)
}
