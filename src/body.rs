use async_std::task::{Context, Poll};
use futures_io::AsyncRead;

use std::fmt;
use std::io;
use std::pin::Pin;

/// A streaming HTTP body.
pub struct Body<R> {
    reader: Option<R>,
}

impl<R> fmt::Debug for Body<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body").finish()
    }
}

impl<R: AsyncRead> Body<R> {
    /// Create a new instance from a reader.
    pub fn new(reader: R) -> Self {
        Self { reader: Some(reader) }
    }

    /// Create a new empty body.
    pub fn empty() -> Self {
        Self {
            reader: None,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Body<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.reader.as_mut() {
            None => Poll::Ready(Ok(0)),
            Some(_reader) => unimplemented!(),
        }
    }
}
