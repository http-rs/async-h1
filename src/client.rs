//! Process HTTP connections on the client.

use async_std::io::{self, BufReader, Read, Write};
use async_std::prelude::*;
use async_std::task::{Context, Poll};
use http_types::{ensure, ensure_eq, format_err};
use http_types::{
    headers::{HeaderName, HeaderValue, CONTENT_LENGTH, DATE, TRANSFER_ENCODING},
    Body, Request, Response, StatusCode,
};

use std::convert::TryFrom;
use std::pin::Pin;
use std::str::FromStr;

use crate::chunked::ChunkedDecoder;
use crate::date::fmt_http_date;
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

/// An HTTP encoder.
#[doc(hidden)]
#[derive(Debug)]
pub struct Encoder {
    /// Keep track how far we've indexed into the headers + body.
    cursor: usize,
    /// HTTP headers to be sent.
    headers: Vec<u8>,
    /// Check whether we're done sending headers.
    headers_done: bool,
    /// Request with the HTTP body to be sent.
    request: Request,
    /// Check whether we're done with the body.
    body_done: bool,
    /// Keep track of how many bytes have been read from the body stream.
    body_bytes_read: usize,
}

impl Encoder {
    /// Create a new instance.
    pub(crate) fn new(headers: Vec<u8>, request: Request) -> Self {
        Self {
            request,
            headers,
            cursor: 0,
            headers_done: false,
            body_done: false,
            body_bytes_read: 0,
        }
    }
}

/// Opens an HTTP/1.1 connection to a remote host.
pub async fn connect<RW>(mut stream: RW, req: Request) -> http_types::Result<Response>
where
    RW: Read + Write + Send + Sync + Unpin + 'static,
{
    let mut req = encode(req).await?;
    log::trace!("> {:?}", &req);

    io::copy(&mut req, &mut stream).await?;

    let res = decode(stream).await?;
    log::trace!("< {:?}", &res);

    Ok(res)
}

/// Encode an HTTP request on the client.
#[doc(hidden)]
async fn encode(req: Request) -> http_types::Result<Encoder> {
    let mut buf: Vec<u8> = vec![];

    let mut url = req.url().path().to_owned();
    if let Some(fragment) = req.url().fragment() {
        url.push('#');
        url.push_str(fragment);
    }
    if let Some(query) = req.url().query() {
        url.push('?');
        url.push_str(query);
    }

    let val = format!("{} {} HTTP/1.1\r\n", req.method(), url);
    log::trace!("> {}", &val);
    buf.write_all(val.as_bytes()).await?;

    // Insert Host header
    // Insert host
    let host = req.url().host_str();
    let host = host.ok_or_else(|| format_err!("Missing hostname"))?;
    let val = if let Some(port) = req.url().port() {
        format!("host: {}:{}\r\n", host, port)
    } else {
        format!("host: {}\r\n", host)
    };

    log::trace!("> {}", &val);
    buf.write_all(val.as_bytes()).await?;

    // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
    // send all items in chunks.
    if let Some(len) = req.len() {
        let val = format!("content-length: {}\r\n", len);
        log::trace!("> {}", &val);
        buf.write_all(val.as_bytes()).await?;
    } else {
        // write!(&mut buf, "Transfer-Encoding: chunked\r\n")?;
        panic!("chunked encoding is not implemented yet");
        // See: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Transfer-Encoding
        //      https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Trailer
    }

    let date = fmt_http_date(std::time::SystemTime::now());
    buf.write_all(b"date: ").await?;
    buf.write_all(date.as_bytes()).await?;
    buf.write_all(b"\r\n").await?;

    for (header, values) in req.iter() {
        for value in values.iter() {
            let val = format!("{}: {}\r\n", header, value);
            log::trace!("> {}", &val);
            buf.write_all(val.as_bytes()).await?;
        }
    }

    buf.write_all(b"\r\n").await?;

    Ok(Encoder::new(buf, req))
}

/// Decode an HTTP response on the client.
#[doc(hidden)]
pub async fn decode<R>(reader: R) -> http_types::Result<Response>
where
    R: Read + Unpin + Send + Sync + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut httparse_res = httparse::Response::new(&mut headers);

    // Keep reading bytes from the stream until we hit the end of the stream.
    loop {
        let bytes_read = reader.read_until(b'\n', &mut buf).await?;
        // No more bytes are yielded from the stream.
        assert!(bytes_read != 0, "Empty response"); // TODO: ensure?

        // Prevent CWE-400 DDOS with large HTTP Headers.
        ensure!(
            buf.len() < MAX_HEAD_LENGTH,
            "Head byte length should be less than 8kb"
        );

        // We've hit the end delimiter of the stream.
        let idx = buf.len() - 1;
        if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
            break;
        }
    }

    // Convert our header buf into an httparse instance, and validate.
    let status = httparse_res.parse(&buf)?;
    ensure!(!status.is_partial(), "Malformed HTTP head");

    let code = httparse_res.code;
    let code = code.ok_or_else(|| format_err!("No status code found"))?;

    // Convert httparse headers + body into a `http_types::Response` type.
    let version = httparse_res.version;
    let version = version.ok_or_else(|| format_err!("No version found"))?;
    ensure_eq!(version, 1, "Unsupported HTTP version");

    let mut res = Response::new(StatusCode::try_from(code)?);
    for header in httparse_res.headers.iter() {
        let name = HeaderName::from_str(header.name)?;
        let value = HeaderValue::from_str(std::str::from_utf8(header.value)?)?;
        res.append_header(name, value)?;
    }

    if res.header(&DATE).is_none() {
        let date = fmt_http_date(std::time::SystemTime::now());
        res.insert_header(DATE, &format!("date: {}\r\n", date)[..])?;
    }

    let content_length = res.header(&CONTENT_LENGTH);
    let transfer_encoding = res.header(&TRANSFER_ENCODING);

    ensure!(
        content_length.is_none() || transfer_encoding.is_none(),
        "Unexpected Content-Length header"
    );

    // Check for Transfer-Encoding
    match transfer_encoding {
        Some(encoding) if !encoding.is_empty() => {
            if encoding.last().unwrap().as_str() == "chunked" {
                let trailers_sender = res.send_trailers();
                let reader = BufReader::new(ChunkedDecoder::new(reader, trailers_sender));
                res.set_body(Body::from_reader(reader, None));
                return Ok(res);
            }
            // Fall through to Content-Length
        }
        _ => {
            // Fall through to Content-Length
        }
    }

    // Check for Content-Length.
    match content_length {
        Some(len) => {
            let len = len.last().unwrap().as_str().parse::<usize>()?;
            res.set_body(Body::from_reader(reader.take(len as u64), Some(len)));
        }
        None => {}
    }

    // Return the response.
    Ok(res)
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
            let inner_poll_result =
                Pin::new(&mut self.request).poll_read(cx, &mut buf[bytes_read..]);
            let n = match inner_poll_result {
                Poll::Ready(Ok(n)) => n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => {
                    if bytes_read == 0 {
                        return Poll::Pending;
                    } else {
                        return Poll::Ready(Ok(bytes_read as usize));
                    }
                }
            };
            bytes_read += n;
            self.body_bytes_read += n;
            if bytes_read == 0 {
                self.body_done = true;
            }
        }

        Poll::Ready(Ok(bytes_read as usize))
    }
}
