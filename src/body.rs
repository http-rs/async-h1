use async_std::task::{Context, Poll};
use futures_io::AsyncRead;

use async_std::io::{self, Cursor};
use std::fmt;
use std::pin::Pin;

/// A streaming HTTP body.
pub struct Body<R: AsyncRead> {
    reader: R,
    length: Option<usize>,
}

impl<R: AsyncRead> fmt::Debug for Body<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body")
            .field("length", &self.length)
            .finish()
    }
}

impl<R: AsyncRead> Body<R> {
    /// Create a new instance from a reader.
    ///
    /// Because the size of a stream is not known ahead of time, we have to use
    /// `Transfer-Encoding: Chunked`.
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            length: None,
        }
    }

    /// Create a new instance from a reader with a known size.
    pub fn new_with_size(reader: R, size: usize) -> Self {
        Self {
            reader,
            length: Some(size),
        }
    }

    /// Create a new empty body.
    pub fn empty(reader: R) -> Self {
        Self {
            reader,
            length: Some(0),
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

impl<R: AsyncRead + Unpin> Body<R> {
    pub fn into_reader(self) -> R {
        self.reader
    }
}

impl Body<io::Cursor<Vec<u8>>> {
    /// Create a new instance from a string.
    #[inline]
    pub fn from_string(string: String) -> Self {
        Self {
            length: Some(string.len()),
            reader: Cursor::new(string.into_bytes()),
        }
    }

    /// Create a new instance from bytes.
    #[inline]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            length: Some(bytes.len()),
            reader: Cursor::new(bytes),
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Body<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}
