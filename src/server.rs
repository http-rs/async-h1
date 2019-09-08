//! Process HTTP connections on the server.

use async_std::io::{self, BufRead, BufReader, Read};
use async_std::task::{Context, Poll};
use futures_io::AsyncRead;
use http::{Request, Response, Version};

use std::pin::Pin;

use crate::{Body, Exception, MAX_HEADERS};

/// A streaming HTTP encoder.
#[derive(Debug)]
pub struct Encoder {
    headers: Vec<u8>,
    headers_done: bool,
    cursor: usize,
    body: Body,
}

impl Encoder {
    /// Create a new instance.
    pub(crate) fn new(headers: Vec<u8>, body: Body) -> Self {
        Self {
            body,
            headers,
            cursor: 0,
            headers_done: false,
        }
    }
}

impl AsyncRead for Encoder {
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
        buf.copy_from_slice(&mut self.headers[range]);
        self.cursor += len;
        if self.cursor >= self.headers.len() {
            self.headers_done = true;
        }
        Poll::Ready(Ok(len as usize))
    }
}

/// Encode an HTTP request on the server.
// TODO: return a reader in the response
pub async fn encode(res: Response<Body>) -> Result<Encoder, std::io::Error> {
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
pub async fn decode(reader: &mut (impl AsyncRead + Unpin)) -> Result<Request<Body>, Exception> {
    let mut reader = BufReader::new(reader);
    let mut request = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);

    loop {
        let bytes_read = reader.read_until(b'\n', &mut request).await?;
        let end = request.len() - 1;
        if bytes_read == 0 || end >= 3 && request[end - 3..=end] == [13, 10, 13, 10] {
            break;
        }
    }
    if req.parse(&request)?.is_partial() {
        return Err("Malformed HTTP header".into());
    }

    let _body = if let Some(header) = req.headers.iter().find(|h| h.name == "Content-Length") {
        let mut body = Vec::new();
        body.resize(std::str::from_utf8(&header.value)?.parse::<usize>()?, 0);
        reader.read_exact(&mut body).await?;
        Some(body)
    } else {
        None
    };

    let request = req;
    let mut req = Request::builder();
    for header in request.headers {
        req.header(header.name, header.value);
    }
    if let Some(method) = request.method {
        req.method(method);
    }
    if let Some(path) = request.path {
        req.uri(path);
    }
    if let Some(version) = request.version {
        req.version(match version {
            1 => Version::HTTP_11,
            _ => return Err("Unsupported HTTP version".into()),
        });
    }

    // TODO: create a body.
    let body = Body {};
    Ok(req.body(body)?)
}
