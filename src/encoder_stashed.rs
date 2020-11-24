use std::pin::Pin;

use async_std::io;
use async_std::io::Read;
use async_std::task::{Context, Poll};
use futures_core::ready;

/// An encoder for chunked encoding.
#[derive(Debug)]
pub(crate) struct ChunkedEncoder<R> {
    reader: R,
}

impl<'a, R: Read + Unpin + 'a> ChunkedEncoder<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self { reader }
    }
}

impl<'a, R: Read + Unpin + 'a> Read for ChunkedEncoder<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut reader = &mut self.reader;
        if buf.len() < 5 {
            return Poll::Pending;
        }
        let max_framing_size = 5 + (buf.len() - 4) / 16;
        let bytes = ready!(Pin::new(&mut reader).poll_read(cx, &mut buf[0..max_framing_size]))?;
        let start = format!("{:X}\r\n", bytes);
        let start_length = start.as_bytes().len();
        let total = bytes + start_length;
        buf.copy_within(..bytes, start_length);
        buf[..start_length].copy_from_slice(start.as_bytes());
        buf[total..total + 2].copy_from_slice(b"\r\n");
        println!("{}", &std::str::from_utf8(&buf[..total + 2]).unwrap());
        Poll::Ready(Ok(total + 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::io::Cursor;
    use async_std::io::ReadExt;
    use http_types::{Body, Result};

    fn rand_string_with_len(n: usize) -> String {
        use rand::distributions::Alphanumeric;
        use rand::{thread_rng, Rng};

        thread_rng().sample_iter(&Alphanumeric).take(n).collect()
    }

    async fn read_to_string_with_buffers_of_size<R: Read + Unpin>(
        mut r: R,
        size: usize,
    ) -> Result<String> {
        let mut total_string = String::new();
        loop {
            let mut buf = vec![0; size];
            let read_bytes = r.read(&mut buf).await?;
            total_string.push_str(std::str::from_utf8(&buf[..read_bytes])?);
            if read_bytes == 0 {
                return Ok(total_string);
            }
        }
    }

    #[async_std::test]
    async fn test() -> Result<()> {
        let body_length = 300;
        let buffer_size = 25;
        let rand_string = rand_string_with_len(body_length);
        let body = Body::from_reader(Cursor::new(rand_string.clone()), None);

        let result = read_to_string_with_buffers_of_size(ChunkedEncoder::new(body), 5).await?;
        assert_eq!(
            result, rand_string,
            "body_length {} buffer_size {}",
            body_length, buffer_size
        );
        Ok(())
    }
}
