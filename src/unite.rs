use core::pin::Pin;
use core::task::{Context, Poll};

use futures_io::{AsyncBufRead, AsyncRead, AsyncWrite};
use pin_project::pin_project;

#[pin_project]
pub(crate) struct Unite<R, W> {
    #[pin]
    reader: R,
    #[pin]
    writer: W,
}

impl<R, W> Unite<R, W> {
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

impl<R: AsyncRead, W> AsyncRead for Unite<R, W> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<futures_io::Result<usize>> {
        self.project().reader.poll_read(cx, buf)
    }
}

impl<R: AsyncBufRead, W> AsyncBufRead for Unite<R, W> {
    fn poll_fill_buf(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<futures_io::Result<&[u8]>> {
        self.project().reader.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.project().reader.consume(amt)
    }
}

impl<R, W: AsyncWrite> AsyncWrite for Unite<R, W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<futures_io::Result<usize>> {
        self.project().writer.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        self.project().writer.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        self.project().writer.poll_close(cx)
    }
}
