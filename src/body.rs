use async_std::task::{Context, Poll};
use futures_io::AsyncRead;

use std::fmt;
use std::io::{self, Cursor};
use std::pin::Pin;

/// A streaming HTTP body.
pub struct Body<R: AsyncRead> {
    reader: Option<R>,
    length: Option<usize>,
}

impl<R: AsyncRead> fmt::Debug for Body<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body").finish()
    }
}

impl<R: AsyncRead> Body<R> {
    /// Create a new instance from a reader.
    pub fn new(reader: R) -> Self {
        Self {
            reader: Some(reader),
            length: None,
        }
    }

    /// Create a new empty body.
    pub fn empty() -> Self {
        Self {
            reader: None,
            length: None,
        }
    }

    /// Get the length of the body, if it has been set.
    ///
    /// The body length is only set for non-streaming types. If a type is streaming the amount of
    /// bytes sent needs to be calculated from the output stream.
    #[inline]
    pub fn len(&self) -> Option<usize> {
        self.length
    }
}

impl Body<io::Cursor<Vec<u8>>> {
    /// Create a new instance from a string.
    #[inline]
    pub fn from_string(string: String) -> Self {
        Self {
            length: Some(string.len()),
            reader: Some(Cursor::new(string.into_bytes())),
        }
    }

    /// Create a new instance from bytes.
    #[inline]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            length: Some(bytes.len()),
            reader: Some(Cursor::new(bytes)),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Body<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.reader.as_mut() {
            None => Poll::Ready(Ok(0)),
            Some(reader) => Pin::new(&mut *reader).poll_read(cx, buf),
        }
    }
}
