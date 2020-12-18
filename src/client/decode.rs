use async_std::io::{BufReader, Read};
use async_std::prelude::*;
use http_types::content::ContentLength;
use http_types::{
    headers::{DATE, TRANSFER_ENCODING},
    Body, Response, StatusCode,
};

use std::convert::TryFrom;

use crate::date::fmt_http_date;
use crate::{chunked::ChunkedDecoder, Error};
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

const CR: u8 = b'\r';
const LF: u8 = b'\n';

/// Decode an HTTP response on the client.
pub async fn decode<R>(reader: R) -> crate::Result<Option<Response>>
where
    R: Read + Unpin + Send + Sync + 'static,
{
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut httparse_res = httparse::Response::new(&mut headers);

    // Keep reading bytes from the stream until we hit the end of the stream.
    loop {
        let bytes_read = reader.read_until(LF, &mut buf).await?;
        // No more bytes are yielded from the stream.
        if bytes_read == 0 {
            return Ok(None);
        }

        // Prevent CWE-400 DDOS with large HTTP Headers.
        if buf.len() >= MAX_HEAD_LENGTH {
            return Err(Error::HeadersTooLong);
        }

        // We've hit the end delimiter of the stream.
        let idx = buf.len() - 1;
        if idx >= 3 && buf[idx - 3..=idx] == [CR, LF, CR, LF] {
            break;
        }
        if idx >= 1 && buf[idx - 1..=idx] == [LF, LF] {
            break;
        }
    }

    // Convert our header buf into an httparse instance, and validate.
    let status = httparse_res.parse(&buf)?;
    if status.is_partial() {
        return Err(Error::PartialHead);
    }

    let code = httparse_res.code.ok_or(Error::MissingStatusCode)?;

    // Convert httparse headers + body into a `http_types::Response` type.
    let version = httparse_res.version.ok_or(Error::MissingVersion)?;

    if version != 1 {
        return Err(Error::UnsupportedVersion(version));
    }

    let status_code =
        StatusCode::try_from(code).map_err(|_| Error::UnrecognizedStatusCode(code))?;
    let mut res = Response::new(status_code);

    for header in httparse_res.headers.iter() {
        res.append_header(header.name, std::str::from_utf8(header.value)?);
    }

    if res.header(DATE).is_none() {
        let date = fmt_http_date(std::time::SystemTime::now());
        res.insert_header(DATE, &format!("date: {}\r\n", date)[..]);
    }

    let content_length =
        ContentLength::from_headers(&res).map_err(|_| Error::MalformedHeader("content-length"))?;
    let transfer_encoding = res.header(TRANSFER_ENCODING);

    if content_length.is_some() && transfer_encoding.is_some() {
        return Err(Error::UnexpectedHeader("content-length"));
    }

    if let Some(encoding) = transfer_encoding {
        if encoding.last().as_str() == "chunked" {
            let trailers_sender = res.send_trailers();
            let reader = BufReader::new(ChunkedDecoder::new(reader, trailers_sender));
            res.set_body(Body::from_reader(reader, None));

            // Return the response.
            return Ok(Some(res));
        }
    }

    // Check for Content-Length.
    if let Some(content_length) = content_length {
        let len = content_length.len();
        res.set_body(Body::from_reader(reader.take(len), Some(len as usize)));
    }

    // Return the response.
    Ok(Some(res))
}
