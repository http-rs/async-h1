//! Process HTTP connections on the server.

use std::str::FromStr;

use async_std::io::{BufReader, Read, Write};
use async_std::prelude::*;
use http_types::headers::{CONTENT_LENGTH, EXPECT, HOST, TRANSFER_ENCODING};
use http_types::{ensure, ensure_eq, format_err};
use http_types::{Body, Method, Request};

use crate::chunked::ChunkedDecoder;
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

const LF: u8 = b'\n';

/// The number returned from httparse when the request is HTTP 1.1
const HTTP_1_1_VERSION: u8 = 1;

/// Decode an HTTP request on the server.
pub(crate) async fn decode<IO>(mut io: IO) -> http_types::Result<Option<Request>>
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

    let path = httparse_req.path;
    let path = path.ok_or_else(|| format_err!("No uri found"))?;

    let version = httparse_req.version;
    let version = version.ok_or_else(|| format_err!("No version found"))?;

    ensure_eq!(
        version,
        HTTP_1_1_VERSION,
        "Unsupported HTTP version 1.{}",
        version
    );

    let mut req = Request::new(
        Method::from_str(method)?,
        url::Url::parse("http://_").unwrap().join(path)?,
    );

    for header in httparse_req.headers.iter() {
        req.insert_header(header.name, std::str::from_utf8(header.value)?);
    }

    set_url_and_port_from_host_header(&mut req)?;
    handle_100_continue(&req, &mut io).await?;

    let content_length = req.header(CONTENT_LENGTH);
    let transfer_encoding = req.header(TRANSFER_ENCODING);

    http_types::ensure!(
        content_length.is_none() || transfer_encoding.is_none(),
        "Unexpected Content-Length header"
    );

    // Check for Transfer-Encoding
    if let Some(encoding) = transfer_encoding {
        if encoding.last().as_str() == "chunked" {
            let trailer_sender = req.send_trailers();
            let reader = BufReader::new(ChunkedDecoder::new(reader, trailer_sender));
            req.set_body(Body::from_reader(reader, None));
            return Ok(Some(req));
        }
        // Fall through to Content-Length
    }

    // Check for Content-Length.
    if let Some(len) = content_length {
        let len = len.last().as_str().parse::<usize>()?;
        req.set_body(Body::from_reader(reader.take(len as u64), Some(len)));
    }

    Ok(Some(req))
}

fn set_url_and_port_from_host_header(req: &mut Request) -> http_types::Result<()> {
    let host = req
        .header(HOST)
        .map(|header| header.last()) // There must only exactly one Host header, so this is permissive
        .ok_or_else(|| format_err!("Mandatory Host header missing"))? //  https://tools.ietf.org/html/rfc7230#section-5.4
        .to_string();

    if let Some(colon) = host.find(":") {
        req.url_mut().set_host(Some(&host[0..colon]))?;
        req.url_mut()
            .set_port(host[colon + 1..].parse().ok())
            .unwrap();
    } else {
        req.url_mut().set_host(Some(&host))?;
    }

    Ok(())
}

const EXPECT_HEADER_VALUE: &str = "100-continue";
const EXPECT_RESPONSE: &[u8] = b"HTTP/1.1 100 Continue\r\n";

async fn handle_100_continue<IO>(req: &Request, io: &mut IO) -> http_types::Result<()>
where
    IO: Write + Unpin,
{
    if let Some(EXPECT_HEADER_VALUE) = req.header(EXPECT).map(|h| h.as_str()) {
        io.write_all(EXPECT_RESPONSE).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_100_continue_does_nothing_with_no_expect_header() {
        let request = Request::new(Method::Get, url::Url::parse("x:").unwrap());
        let mut io = async_std::io::Cursor::new(vec![]);
        let result = async_std::task::block_on(handle_100_continue(&request, &mut io));
        assert_eq!(std::str::from_utf8(&io.into_inner()).unwrap(), "");
        assert!(result.is_ok());
    }

    #[test]
    fn handle_100_continue_sends_header_if_expects_is_exactly_right() {
        let mut request = Request::new(Method::Get, url::Url::parse("x:").unwrap());
        request.append_header("expect", "100-continue");
        let mut io = async_std::io::Cursor::new(vec![]);
        let result = async_std::task::block_on(handle_100_continue(&request, &mut io));
        assert_eq!(
            std::str::from_utf8(&io.into_inner()).unwrap(),
            "HTTP/1.1 100 Continue\r\n"
        );
        assert!(result.is_ok());
    }

    #[test]
    fn handle_100_continue_does_nothing_if_expects_header_is_wrong() {
        let mut request = Request::new(Method::Get, url::Url::parse("x:").unwrap());
        request.append_header("expect", "110-extensions-not-allowed");
        let mut io = async_std::io::Cursor::new(vec![]);
        let result = async_std::task::block_on(handle_100_continue(&request, &mut io));
        assert_eq!(std::str::from_utf8(&io.into_inner()).unwrap(), "");
        assert!(result.is_ok());
    }

    #[test]
    fn test_setting_host_with_no_port() {
        let mut request = request_with_host_header("subdomain.mydomain.tld");
        set_url_and_port_from_host_header(&mut request).unwrap();
        assert_eq!(
            request.url(),
            &url::Url::parse("http://subdomain.mydomain.tld/some/path").unwrap()
        );
    }

    #[test]
    fn test_setting_host_with_a_port() {
        let mut request = request_with_host_header("subdomain.mydomain.tld:8080");
        set_url_and_port_from_host_header(&mut request).unwrap();
        assert_eq!(
            request.url(),
            &url::Url::parse("http://subdomain.mydomain.tld:8080/some/path").unwrap()
        );
    }

    #[test]
    fn test_setting_host_with_an_ip_and_port() {
        let mut request = request_with_host_header("12.34.56.78:90");
        set_url_and_port_from_host_header(&mut request).unwrap();
        assert_eq!(
            request.url(),
            &url::Url::parse("http://12.34.56.78:90/some/path").unwrap()
        );
    }

    #[test]
    fn test_malformed_nonnumeric_port_is_ignored() {
        let mut request = request_with_host_header("hello.world:uh-oh");
        set_url_and_port_from_host_header(&mut request).unwrap();
        assert_eq!(
            request.url(),
            &url::Url::parse("http://hello.world/some/path").unwrap()
        );
    }

    #[test]
    fn test_malformed_trailing_colon_is_ignored() {
        let mut request = request_with_host_header("edge.cases:");
        set_url_and_port_from_host_header(&mut request).unwrap();
        assert_eq!(
            request.url(),
            &url::Url::parse("http://edge.cases/some/path").unwrap()
        );
    }

    #[test]
    fn test_malformed_leading_colon_is_invalid_host_value() {
        let mut request = request_with_host_header(":300");
        assert!(set_url_and_port_from_host_header(&mut request).is_err());
    }

    #[test]
    fn test_malformed_invalid_url_host_is_invalid_host_header_value() {
        let mut request = request_with_host_header(" ");
        assert!(set_url_and_port_from_host_header(&mut request).is_err());
    }

    fn request_with_host_header(host: &str) -> Request {
        let mut req = Request::new(Method::Get, url::Url::parse("http://_/some/path").unwrap());
        req.insert_header(HOST, host);
        req
    }
}
