use async_std::io::{self, BufRead, Read};
use std::pin::Pin;
use std::task::{Context, Poll};

pin_project_lite::pin_project! {
    pub struct ChunkedDecoder<R> {
        #[pin]
        reader: R,
    }
}

/// Decodes a chunked body according to
/// https://tools.ietf.org/html/rfc7230#section-4.1
impl<R> ChunkedDecoder<R> {
    pub fn new(reader: R) -> Self {
        ChunkedDecoder { reader }
    }
}

impl<R: BufRead + Unpin + Send + 'static> Read for ChunkedDecoder<R> {
    #[allow(missing_doc_code_examples)]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        this.reader.poll_read(cx, buf)
    }
}

impl<R: BufRead + Unpin + Send + 'static> BufRead for ChunkedDecoder<R> {
    #[allow(missing_doc_code_examples)]
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&'_ [u8]>> {
        let this = self.project();
        this.reader.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        this.reader.consume(amt);
    }
}
