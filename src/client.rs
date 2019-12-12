//! Process HTTP connections on the client.

use async_std::io::{self, BufReader};
use async_std::prelude::*;
use async_std::task::{Context, Poll};
use futures_core::ready;
use futures_io::AsyncRead;
use http_types::{
    headers::{HeaderName, HeaderValue, CONTENT_LENGTH},
    Body, Request, Response, StatusCode,
};

use std::pin::Pin;
use std::str::FromStr;

use crate::{Exception, MAX_HEADERS};

/// An HTTP encoder.
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

/// Encode an HTTP request on the client.
pub async fn encode(req: Request) -> Result<Encoder, std::io::Error> {
    let mut buf: Vec<u8> = vec![];

    write!(&mut buf, "{} {} HTTP/1.1\r\n", req.method(), req.url(),).await?;

    // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
    // send all items in chunks.
    if let Some(len) = req.len() {
        write!(&mut buf, "Content-Length: {}\r\n", len).await?;
    } else {
        // write!(&mut buf, "Transfer-Encoding: chunked\r\n")?;
        panic!("chunked encoding is not implemented yet");
        // See: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Transfer-Encoding
        //      https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Trailer
    }
    for (header, values) in req.iter() {
        for value in values.iter() {
            write!(&mut buf, "{}: {}\r\n", header, value).await?;
        }
    }

    write!(&mut buf, "\r\n").await?;
    Ok(Encoder::new(buf, req))
}

/// Decode an HTTP respons on the client.
pub async fn decode<R>(reader: R) -> Result<Response, Exception>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut httparse_res = httparse::Response::new(&mut headers);

    // Keep reading bytes from the stream until we hit the end of the stream.
    loop {
        let bytes_read = reader.read_until(b'\n', &mut buf).await?;
        // No more bytes are yielded from the stream.
        if bytes_read == 0 {
            panic!("empty response");
        }

        // We've hit the end delimiter of the stream.
        let idx = buf.len() - 1;
        if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
            break;
        }
    }

    // Convert our header buf into an httparse instance, and validate.
    let status = httparse_res.parse(&buf)?;
    if status.is_partial() {
        dbg!(String::from_utf8(buf).unwrap());
        return Err("Malformed HTTP head".into());
    }
    let code = httparse_res.code.ok_or_else(|| "No status code found")?;

    // Convert httparse headers + body into a `http::Response` type.
    let version = httparse_res.version.ok_or_else(|| "No version found")?;
    if version != 1 {
        return Err("Unsupported HTTP version".into());
    }
    use std::convert::TryFrom;
    let mut res = Response::new(StatusCode::try_from(code)?);
    for header in httparse_res.headers.iter() {
        let name = HeaderName::from_str(header.name)?;
        let value = HeaderValue::from_str(std::str::from_utf8(header.value)?)?;
        res.insert_header(name, value)?;
    }

    // Process the body if `Content-Length` was passed.
    if let Some(content_length) = res.header(&CONTENT_LENGTH) {
        let length = content_length
            .last()
            .unwrap()
            .as_str()
            .parse::<usize>()
            .ok();

        if let Some(len) = length {
            res.set_body(Body::from_reader(reader));
            res.set_len(len);
        } else {
            return Err("Invalid value for Content-Length".into());
        }
    };

    // Return the response.
    Ok(res)
}

impl AsyncRead for Encoder {
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
            let n = ready!(Pin::new(&mut self.request).poll_read(cx, &mut buf[bytes_read..]))?;
            bytes_read += n;
            self.body_bytes_read += n;
            if bytes_read == 0 {
                self.body_done = true;
            }
        }

        Poll::Ready(Ok(bytes_read as usize))
    }
}
