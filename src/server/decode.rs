//! Process HTTP connections on the server.

use std::str::FromStr;

use async_std::io::{BufReader, Read, Write};
use async_std::{prelude::*, sync, task};
use http_types::headers::{CONTENT_LENGTH, EXPECT, TRANSFER_ENCODING};
use http_types::{ensure, ensure_eq, format_err};
use http_types::{Body, Method, Request, Url};

use crate::chunked::ChunkedDecoder;
use crate::read_notifier::ReadNotifier;
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

const LF: u8 = b'\n';

/// The number returned from httparse when the request is HTTP 1.1
const HTTP_1_1_VERSION: u8 = 1;

const CONTINUE_HEADER_VALUE: &str = "100-continue";
const CONTINUE_RESPONSE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";

/// Decode an HTTP request on the server.
pub async fn decode<IO>(mut io: IO) -> http_types::Result<Option<Request>>
where
    IO: Read + Write + Clone + Send + Sync + Unpin + 'static,
{
    let mut reader = BufReader::new(io.clone());
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

    let version = httparse_req.version;
    let version = version.ok_or_else(|| format_err!("No version found"))?;

    ensure_eq!(
        version,
        HTTP_1_1_VERSION,
        "Unsupported HTTP version 1.{}",
        version
    );

    let url = url_from_httparse_req(&httparse_req)?;

    let mut req = Request::new(Method::from_str(method)?, url);

    req.set_version(Some(http_types::Version::Http1_1));

    for header in httparse_req.headers.iter() {
        req.insert_header(header.name, std::str::from_utf8(header.value)?);
    }

    let content_length = req.header(CONTENT_LENGTH);
    let transfer_encoding = req.header(TRANSFER_ENCODING);

    http_types::ensure!(
        content_length.is_none() || transfer_encoding.is_none(),
        "Unexpected Content-Length header"
    );

    let (sender, receiver) = sync::channel(1);

    if let Some(CONTINUE_HEADER_VALUE) = req.header(EXPECT).map(|h| h.as_str()) {
        task::spawn(async move {
            if let Ok(()) = receiver.recv().await {
                io.write_all(CONTINUE_RESPONSE).await.ok();
            };
        });
    }

    // Check for Transfer-Encoding
    if let Some(encoding) = transfer_encoding {
        if encoding.last().as_str() == "chunked" {
            let trailer_sender = req.send_trailers();
            let reader = BufReader::new(ChunkedDecoder::new(reader, trailer_sender));
            let reader = ReadNotifier::new(reader, sender);
            req.set_body(Body::from_reader(reader, None));
            return Ok(Some(req));
        }
        // Fall through to Content-Length
    }

    // Check for Content-Length.
    if let Some(len) = content_length {
        let len = len.last().as_str().parse::<usize>()?;
        let reader = ReadNotifier::new(reader.take(len as u64), sender);
        req.set_body(Body::from_reader(reader, Some(len)));
    }

    Ok(Some(req))
}

fn url_from_httparse_req(req: &httparse::Request<'_, '_>) -> http_types::Result<Url> {
    let path = req.path.ok_or_else(|| format_err!("No uri found"))?;
    let host = req
        .headers
        .iter()
        .filter(|x| x.name.eq_ignore_ascii_case("host"))
        .next()
        .ok_or_else(|| format_err!("Mandatory Host header missing"))?
        .value;

    let host = std::str::from_utf8(host)?;

    if path.starts_with("http://") || path.starts_with("https://") {
        Ok(Url::parse(path)?)
    } else if path.starts_with("/") {
        Ok(Url::parse(&format!("http://{}/", host))?.join(path)?)
    } else if req.method.unwrap().eq_ignore_ascii_case("connect") {
        Ok(Url::parse(&format!("http://{}/", path))?)
    } else {
        Err(format_err!("unexpected uri format"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn httparse_req(buf: &str, f: impl Fn(httparse::Request<'_, '_>)) {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut res = httparse::Request::new(&mut headers[..]);
        res.parse(buf.as_bytes()).unwrap();
        f(res)
    }

    #[test]
    fn url_for_connect() {
        httparse_req(
            "CONNECT server.example.com:443 HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/");
            },
        );
    }

    #[test]
    fn url_for_host_plus_path() {
        httparse_req(
            "GET /some/resource HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/some/resource");
            },
        )
    }

    #[test]
    fn url_for_host_plus_absolute_url() {
        httparse_req(
            "GET http://domain.com/some/resource HTTP/1.1\r\nHost: server.example.com\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://domain.com/some/resource"); // host header MUST be ignored according to spec
            },
        )
    }

    #[test]
    fn url_for_conflicting_connect() {
        httparse_req(
            "CONNECT server.example.com:443 HTTP/1.1\r\nHost: conflicting.host\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/");
            },
        )
    }

    #[test]
    fn url_for_malformed_resource_path() {
        httparse_req(
            "GET not-a-url HTTP/1.1\r\nHost: server.example.com\r\n",
            |req| {
                assert!(url_from_httparse_req(&req).is_err());
            },
        )
    }
}
