//! Process HTTP connections on the server.

use std::str::FromStr;

use async_std::io::BufReader;
use async_std::io::Read;
use async_std::prelude::*;
use http_types::headers::{HeaderName, HeaderValue, CONTENT_LENGTH, TRANSFER_ENCODING};
use http_types::{ensure, ensure_eq, format_err};
use http_types::{Body, Method, Request};

use crate::chunked::ChunkedDecoder;
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

const LF: u8 = b'\n';

/// The number returned from httparse when the request is HTTP 1.1
const HTTP_1_1_VERSION: u8 = 1;

/// Decode an HTTP request on the server.
pub(crate) async fn decode<R>(addr: &str, reader: R) -> http_types::Result<Option<Request>>
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
            "Head byte length should be less than 8kb"
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
