//! Process HTTP connections on the server.

use async_std::future::Future;
use async_std::io::{self, BufReader};
use async_std::io::{Read, Write};
use async_std::prelude::*;
use async_std::task::{Context, Poll};
use futures_core::ready;
use http::{Request, Response, Version};
use std::time::{Duration, Instant};

use std::pin::Pin;

use crate::{Body, Exception, MAX_HEADERS};

pub async fn connect<'a, F, Fut, R, W, O: 'a>(
    reader: &'a mut R,
    writer: &'a mut W,
    callback: F,
) -> Result<(), Exception>
where
    R: Read + Unpin + Send,
    W: Write + Unpin,
    F: Fn(&mut Request<Body<BufReader<&'a mut R>>>) -> Fut,
    Fut: Future<Output = Result<Response<Body<O>>, Exception>>,
    O: Read + Unpin + Send,
{
    let req = decode(reader).await?;
    if let RequestOrReader::Request(mut req) = req {
        let headers = req.headers();
        let timeout = match (headers.get("Connection"), headers.get("Keep-Alive")) {
            (Some(connection), Some(_v))
                if connection == http::header::HeaderValue::from_static("Keep-Alive") =>
            {
                // TODO: parse timeout
                Duration::from_secs(5)
            }
            _ => Duration::from_secs(5),
        };

        let beginning = Instant::now();
        loop {
            // TODO: what to do when the callback returns Err
            let mut res = encode(callback(&mut req).await?).await?;
            io::copy(&mut res, writer).await?;
            let mut stream = req.into_body().into_reader().into_inner();
            req = loop {
                match decode(stream).await? {
                    RequestOrReader::Request(r) => break r,
                    RequestOrReader::Reader(r) => {
                        let now = Instant::now();
                        if now - beginning > timeout {
                            return Ok(());
                        }
                        stream = r;
                    }
                }
            };
        }
    }

    Ok(())
}

/// A streaming HTTP encoder.
///
/// This is returned from [`encode`].
#[derive(Debug)]
pub struct Encoder<R: Read> {
    /// Keep track how far we've indexed into the headers + body.
    cursor: usize,
    /// HTTP headers to be sent.
    headers: Vec<u8>,
    /// Check whether we're done sending headers.
    headers_done: bool,
    /// HTTP body to be sent.
    body: Body<R>,
    /// Check whether we're done with the body.
    body_done: bool,
    /// Keep track of how many bytes have been read from the body stream.
    body_bytes_read: usize,
}

impl<R: Read> Encoder<R> {
    /// Create a new instance.
    pub(crate) fn new(headers: Vec<u8>, body: Body<R>) -> Self {
        Self {
            body,
            headers,
            cursor: 0,
            headers_done: false,
            body_done: false,
            body_bytes_read: 0,
        }
    }
}

impl<R: Read + Unpin> Read for Encoder<R> {
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
            let n = ready!(Pin::new(&mut self.body).poll_read(cx, &mut buf[bytes_read..]))?;
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
pub async fn encode<R>(res: Response<Body<R>>) -> io::Result<Encoder<R>>
where
    R: Read + Send,
{
    let mut buf: Vec<u8> = vec![];

    let reason = res.status().canonical_reason().unwrap();
    let status = res.status();
    std::io::Write::write_fmt(
        &mut buf,
        format_args!("HTTP/1.1 {} {}\r\n", status.as_str(), reason),
    )?;

    // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
    // send all items in chunks.
    if let Some(len) = res.body().len() {
        std::io::Write::write_fmt(&mut buf, format_args!("Content-Length: {}\r\n", len))?;
    } else {
        std::io::Write::write_fmt(&mut buf, format_args!("Transfer-Encoding: chunked\r\n"))?;
        panic!("chunked encoding is not implemented yet");
        // See: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Transfer-Encoding
        //      https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Trailer
    }

    for (header, value) in res.headers() {
        std::io::Write::write_fmt(
            &mut buf,
            format_args!("{}: {}\r\n", header.as_str(), value.to_str().unwrap()),
        )?
    }

    std::io::Write::write_fmt(&mut buf, format_args!("\r\n"))?;
    Ok(Encoder::new(buf, res.into_body()))
}

#[derive(Debug)]
pub enum RequestOrReader<R: Read> {
    Request(Request<Body<BufReader<R>>>),
    Reader(R),
}

/// Decode an HTTP request on the server.
pub async fn decode<R>(reader: R) -> Result<RequestOrReader<R>, Exception>
where
    R: Read + Unpin + Send,
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
            return Ok(RequestOrReader::Reader(reader.into_inner()));
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
    let body = match httparse_req
        .headers
        .iter()
        .find(|h| h.name == "Content-Length")
    {
        Some(_header) => Body::new(reader), // TODO: use the header value
        None => Body::empty(reader),
    };

    // Return the request.
    Ok(RequestOrReader::Request(req.body(body)?))
}
