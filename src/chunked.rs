use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};

use async_std::io::{self, Read};
use async_std::sync::{channel, Arc, Receiver, Sender};
use byte_pool::{Block, BytePool};
use http_types::headers::{HeaderName, HeaderValue};

const INITIAL_CAPACITY: usize = 1024 * 4;
const MAX_CAPACITY: usize = 512 * 1024 * 1024; // 512 MiB

lazy_static::lazy_static! {
    /// The global buffer pool we use for storing incoming data.
    pub(crate) static ref POOL: Arc<BytePool> = Arc::new(BytePool::new());
}

/// Decodes a chunked body according to
/// https://tools.ietf.org/html/rfc7230#section-4.1
pub struct ChunkedDecoder<R: Read> {
    /// The underlying stream
    inner: R,
    /// Buffer for the already read, but not yet parsed data.
    buffer: Block<'static>,
    /// Position of valid read data into buffer.
    current: Position,
    /// Whether we should attempt to decode whatever is currently inside the buffer.
    /// False indicates that we know for certain that the buffer is incomplete.
    initial_decode: bool,
    /// Current state.
    state: State,
    /// Trailer channel sender.
    trailer_sender: Sender<Vec<(HeaderName, HeaderValue)>>,
    /// Trailer channel receiver.
    trailer_receiver: Receiver<Vec<(HeaderName, HeaderValue)>>,
}

impl<R: Read> ChunkedDecoder<R> {
    pub fn new(inner: R) -> Self {
        let (sender, receiver) = channel(1);

        ChunkedDecoder {
            inner,
            buffer: POOL.alloc(INITIAL_CAPACITY),
            current: Position::default(),
            initial_decode: false, // buffer is empty initially, nothing to decode}
            state: State::Init,
            trailer_sender: sender,
            trailer_receiver: receiver,
        }
    }

    pub fn trailer(&self) -> Receiver<Vec<(HeaderName, HeaderValue)>> {
        self.trailer_receiver.clone()
    }
}

fn decode_init(buffer: Block<'static>, pos: &Position) -> io::Result<DecodeResult> {
    use httparse::Status;
    match httparse::parse_chunk_size(&buffer[pos.start..pos.end]) {
        Ok(Status::Complete((used, chunk_len))) => {
            let new_pos = Position {
                start: pos.start + used,
                end: pos.end,
            };

            let new_state = if chunk_len == 0 {
                State::Trailer
            } else {
                State::Chunk(0, chunk_len)
            };

            Ok(DecodeResult::Some {
                read: 0,
                buffer,
                new_pos,
                new_state,
                pending: false,
            })
        }
        Ok(Status::Partial) => Ok(DecodeResult::None(buffer)),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
    }
}

fn decode_chunk_end(buffer: Block<'static>, pos: &Position) -> io::Result<DecodeResult> {
    if pos.len() < 2 {
        return Ok(DecodeResult::None(buffer));
    }

    if &buffer[pos.start..pos.start + 2] == b"\r\n" {
        // valid chunk end move on to a new header
        return Ok(DecodeResult::Some {
            read: 0,
            buffer,
            new_pos: Position {
                start: pos.start + 2,
                end: pos.end,
            },
            new_state: State::Init,
            pending: false,
        });
    }

    Err(io::Error::from(io::ErrorKind::InvalidData))
}

fn decode_trailer(buffer: Block<'static>, pos: &Position) -> io::Result<DecodeResult> {
    use httparse::Status;

    // read headers
    let mut headers = [httparse::EMPTY_HEADER; 16];

    match httparse::parse_headers(&buffer[pos.start..pos.end], &mut headers) {
        Ok(Status::Complete((used, headers))) => {
            let headers = headers
                .iter()
                .map(|header| {
                    // TODO: error propagation
                    let name = HeaderName::from_str(header.name).unwrap();
                    let value =
                        HeaderValue::from_str(&std::string::String::from_utf8_lossy(header.value))
                            .unwrap();
                    (name, value)
                })
                .collect();
            Ok(DecodeResult::Some {
                read: 0,
                buffer,
                new_state: State::TrailerDone(headers),
                new_pos: Position {
                    start: pos.start + used,
                    end: pos.end,
                },
                pending: false,
            })
        }
        Ok(Status::Partial) => Ok(DecodeResult::None(buffer)),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
    }
}

impl<R: Read + Unpin> ChunkedDecoder<R> {
    fn poll_read_chunk(
        &mut self,
        cx: &mut Context<'_>,
        buffer: Block<'static>,
        pos: &Position,
        buf: &mut [u8],
        current: u64,
        len: u64,
    ) -> io::Result<DecodeResult> {
        let mut new_pos = pos.clone();
        let remaining = (len - current) as usize;
        let to_read = std::cmp::min(remaining, buf.len());

        let mut new_current = current;

        // position into buf
        let mut read = 0;

        // first drain the buffer
        if new_pos.len() > 0 {
            let to_read_buf = std::cmp::min(to_read, pos.len());
            buf[..to_read_buf].copy_from_slice(&buffer[new_pos.start..new_pos.start + to_read_buf]);

            new_pos.start += to_read_buf;
            new_current += to_read_buf as u64;
            read += to_read_buf;

            let new_state = if new_current == len {
                State::ChunkEnd
            } else {
                State::Chunk(new_current, len)
            };

            return Ok(DecodeResult::Some {
                read,
                new_state,
                new_pos,
                buffer,
                pending: false,
            });
        }

        match Pin::new(&mut self.inner).poll_read(cx, &mut buf[read..read + to_read]) {
            Poll::Ready(val) => {
                let n = val?;
                new_current += n as u64;
                read += n;
                let new_state = if new_current == len {
                    State::ChunkEnd
                } else {
                    State::Chunk(new_current, len)
                };

                Ok(DecodeResult::Some {
                    read,
                    new_state,
                    new_pos,
                    buffer,
                    pending: false,
                })
            }
            Poll::Pending => {
                return Ok(DecodeResult::Some {
                    read: 0,
                    new_state: State::Chunk(new_current, len),
                    new_pos,
                    buffer,
                    pending: true,
                });
            }
        }
    }

    fn poll_read_inner(
        &mut self,
        cx: &mut Context<'_>,
        buffer: Block<'static>,
        pos: &Position,
        buf: &mut [u8],
    ) -> io::Result<DecodeResult> {
        match self.state {
            State::Init => {
                // Initial read
                decode_init(buffer, pos)
            }
            State::Chunk(current, len) => {
                // reading a chunk
                self.poll_read_chunk(cx, buffer, pos, buf, current, len)
            }
            State::ChunkEnd => decode_chunk_end(buffer, pos),
            State::Trailer => {
                // reading the trailer headers
                decode_trailer(buffer, pos)
            }
            State::TrailerDone(ref mut headers) => {
                let headers = std::mem::replace(headers, Vec::new());
                let mut fut = Box::pin(self.trailer_sender.send(headers));
                match Pin::new(&mut fut).poll(cx) {
                    Poll::Ready(_) => {}
                    Poll::Pending => {
                        return Ok(DecodeResult::Some {
                            read: 0,
                            new_state: self.state.clone(),
                            new_pos: pos.clone(),
                            buffer,
                            pending: true,
                        });
                    }
                }

                Ok(DecodeResult::Some {
                    read: 0,
                    new_state: State::Done,
                    new_pos: pos.clone(),
                    buffer,
                    pending: false,
                })
            }
            State::Done => Ok(DecodeResult::Some {
                read: 0,
                new_state: State::Done,
                new_pos: pos.clone(),
                buffer,
                pending: false,
            }),
        }
    }
}

impl<R: Read + Unpin> Read for ChunkedDecoder<R> {
    #[allow(missing_doc_code_examples)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;

        let mut n = std::mem::replace(&mut this.current, Position::default());
        let buffer = std::mem::replace(&mut this.buffer, POOL.alloc(INITIAL_CAPACITY));
        let mut needs_read = if let State::Chunk(_, _) = this.state {
            false // Do not attempt to fill the buffer when we are reading a chunk
        } else {
            true
        };

        let mut buffer = if n.len() > 0 && this.initial_decode {
            match this.poll_read_inner(cx, buffer, &n, buf)? {
                DecodeResult::Some {
                    read,
                    buffer,
                    new_pos,
                    new_state,
                    pending,
                } => {
                    this.current = new_pos.clone();
                    std::mem::replace(&mut this.state, new_state);

                    if pending {
                        // initial_decode is still true
                        std::mem::replace(&mut this.buffer, buffer);
                        return Poll::Pending;
                    }

                    if State::Done == this.state || read > 0 {
                        // initial_decode is still true
                        std::mem::replace(&mut this.buffer, buffer);
                        return Poll::Ready(Ok(read));
                    }

                    n = new_pos;
                    needs_read = false;
                    buffer
                }
                DecodeResult::None(buffer) => buffer,
            }
        } else {
            buffer
        };

        loop {
            if n.len() >= buffer.capacity() {
                if buffer.capacity() + 1024 <= MAX_CAPACITY {
                    buffer.realloc(buffer.capacity() + 1024);
                } else {
                    std::mem::replace(&mut this.buffer, buffer);
                    this.current = n;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::Other,
                        "incoming data too large",
                    )));
                }
            }

            if needs_read {
                let bytes_read = match Pin::new(&mut this.inner).poll_read(cx, &mut buffer[n.end..])
                {
                    Poll::Ready(result) => result?,
                    Poll::Pending => {
                        // if we're here, it means that we need more data but there is none yet,
                        // so no decoding attempts are necessary until we get more data
                        this.initial_decode = false;

                        std::mem::replace(&mut this.buffer, buffer);
                        this.current = n;
                        return Poll::Pending;
                    }
                };
                n.end += bytes_read;
            }
            match this.poll_read_inner(cx, buffer, &n, buf)? {
                DecodeResult::Some {
                    read,
                    buffer: new_buffer,
                    new_pos,
                    new_state,
                    pending,
                } => {
                    // current buffer might now contain more data inside, so we need to attempt
                    // to decode it next time
                    this.initial_decode = true;
                    std::mem::replace(&mut this.state, new_state);
                    this.current = new_pos;
                    n = new_pos;

                    if State::Done == this.state || read > 0 {
                        std::mem::replace(&mut this.buffer, new_buffer);
                        return Poll::Ready(Ok(read));
                    }

                    if pending {
                        std::mem::replace(&mut this.buffer, new_buffer);
                        return Poll::Pending;
                    }

                    buffer = new_buffer;
                    needs_read = false;
                    continue;
                }
                DecodeResult::None(buf) => {
                    buffer = buf;

                    if this.buffer.is_empty() || n.is_zero() {
                        // "logical buffer" is empty, there is nothing to decode on the next step
                        this.initial_decode = false;

                        std::mem::replace(&mut this.buffer, buffer);
                        this.current = n;

                        return Poll::Ready(Ok(0));
                    } else {
                        needs_read = true;
                    }
                }
            }
        }
    }
}

/// A semantically explicit slice of a buffer.
#[derive(Eq, PartialEq, Debug, Copy, Clone, Default)]
struct Position {
    start: usize,
    end: usize,
}

impl Position {
    fn len(&self) -> usize {
        self.end - self.start
    }

    fn is_zero(&self) -> bool {
        self.start == 0 && self.end == 0
    }
}

enum DecodeResult {
    Some {
        /// How much was read
        read: usize,
        /// Remaining data.
        buffer: Block<'static>,
        new_pos: Position,
        new_state: State,
        pending: bool,
    },
    None(Block<'static>),
}

#[derive(Debug, PartialEq, Clone)]
enum State {
    Init,
    Chunk(u64, u64),
    ChunkEnd,
    Trailer,
    TrailerDone(Vec<(HeaderName, HeaderValue)>),
    Done,
}

impl fmt::Debug for DecodeResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeResult::Some {
                read,
                buffer,
                new_pos,
                new_state,
                pending,
            } => f
                .debug_struct("DecodeResult::Some")
                .field("read", read)
                .field("block", &buffer.len())
                .field("new_pos", new_pos)
                .field("new_state", new_state)
                .field("pending", pending)
                .finish(),
            DecodeResult::None(block) => write!(f, "DecodeResult::None({})", block.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_std::prelude::*;

    #[test]
    fn test_chunked_wiki() {
        async_std::task::block_on(async move {
            let input = async_std::io::Cursor::new(
                "4\r\n\
                  Wiki\r\n\
                  5\r\n\
                  pedia\r\n\
                  E\r\n in\r\n\
                  \r\n\
                  chunks.\r\n\
                  0\r\n\
                  \r\n"
                    .as_bytes(),
            );
            let mut decoder = ChunkedDecoder::new(input);

            let mut output = String::new();
            decoder.read_to_string(&mut output).await.unwrap();
            assert_eq!(
                output,
                "Wikipedia in\r\n\
                 \r\n\
                 chunks."
            );
        });
    }

    #[test]
    fn test_chunked_mdn() {
        async_std::task::block_on(async move {
            let input = async_std::io::Cursor::new(
                "7\r\n\
                 Mozilla\r\n\
                 9\r\n\
                 Developer\r\n\
                 7\r\n\
                 Network\r\n\
                 0\r\n\
                 Expires: Wed, 21 Oct 2015 07:28:00 GMT\r\n\
                 \r\n"
                    .as_bytes(),
            );
            let mut decoder = ChunkedDecoder::new(input);

            let mut output = String::new();
            decoder.read_to_string(&mut output).await.unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork");

            let trailer = decoder.trailer().recv().await;
            assert_eq!(
                trailer,
                Some(vec![(
                    "Expires".parse().unwrap(),
                    "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
                )])
            );
        });
    }
}
