use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};

use async_std::io::{self, BufRead, Read};
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
pub struct ChunkedDecoder<R: BufRead> {
    /// The underlying stream
    inner: R,
    /// Buffer for the already read, but not yet parsed data.
    buffer: Block<'static>,
    /// Position of valid read data into buffer.
    current: Position,
    /// How many bytes do we need to finishe the currrent element that is being decoded.
    decode_needs: usize,
    /// Whether we should attempt to decode whatever is currently inside the buffer.
    /// False indicates that we know for certain that the buffer is incomplete.
    initial_decode: bool,
    /// Current state.
    state: State,
    /// Trailer channel sender.
    trailer_sender: Sender<(HeaderName, HeaderValue)>,
    /// Trailer channel receiver.
    trailer_receiver: Receiver<(HeaderName, HeaderValue)>,
}

impl<R: BufRead> ChunkedDecoder<R> {
    pub fn new(inner: R) -> Self {
        let (sender, receiver) = channel(1);

        ChunkedDecoder {
            inner,
            buffer: POOL.alloc(INITIAL_CAPACITY),
            current: Position::default(),
            decode_needs: 0,
            initial_decode: false, // buffer is empty initially, nothing to decode}
            state: State::Init,
            trailer_sender: sender,
            trailer_receiver: receiver,
        }
    }

    pub fn trailer(&self) -> Receiver<(HeaderName, HeaderValue)> {
        self.trailer_receiver.clone()
    }
}

fn decode_init(buffer: Block<'static>, pos: &Position, buf: &mut [u8]) -> io::Result<DecodeResult> {
    use httparse::Status;

    match httparse::parse_chunk_size(&buffer[pos.start..pos.end]) {
        Ok(Status::Complete((used, chunk_len))) => {
            let new_pos = Position {
                start: pos.start + used,
                end: pos.end,
            };

            if chunk_len == 0 {
                // TODO: decode last_chunk
                decode_trailer(buffer, &new_pos)
            } else {
                decode_chunk(buffer, &new_pos, buf, 0, chunk_len)
            }
        }
        Ok(Status::Partial) => Ok(DecodeResult::None(buffer)),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
    }
}

fn decode_chunk(
    buffer: Block<'static>,
    pos: &Position,
    buf: &mut [u8],
    current: u64,
    len: u64,
) -> io::Result<DecodeResult> {
    let left_to_read = (len - current) as usize;
    let available = std::cmp::min(pos.len(), buf.len());
    let to_read = std::cmp::min(left_to_read, available);

    buf[..to_read].copy_from_slice(&buffer[pos.start..pos.start + to_read]);

    dbg!(to_read);
    dbg!(std::str::from_utf8(&buf[..to_read]));

    let new_pos = Position {
        start: pos.start + to_read,
        end: pos.end,
    };
    let new_state = if left_to_read - to_read > 0 {
        State::Chunk(current + to_read as u64, len)
    } else {
        // read one \r\n
        State::ChunkEnd
    };

    Ok(DecodeResult::Some {
        read: to_read,
        buffer,
        new_state,
        new_pos,
    })
}

fn decode_chunk_end(
    buffer: Block<'static>,
    pos: &Position,
    buf: &mut [u8],
) -> io::Result<DecodeResult> {
    if pos.len() < 2 {
        return Ok(DecodeResult::None(buffer));
    }

    if &buffer[pos.start..pos.start + 2] == b"\r\n" {
        // valid chunk end move on to a new header
        return decode_init(
            buffer,
            &Position {
                start: pos.start + 2,
                end: pos.end,
            },
            buf,
        );
    }

    dbg!(std::str::from_utf8(&buffer[pos.start..pos.end]));

    Err(io::Error::from(io::ErrorKind::InvalidData))
}

fn decode_trailer(buffer: Block<'static>, pos: &Position) -> io::Result<DecodeResult> {
    use httparse::Status;

    // read headers
    let mut headers = [httparse::EMPTY_HEADER; 16];
    dbg!(std::str::from_utf8(&buffer[pos.start..pos.end]));
    match httparse::parse_headers(&buffer[pos.start..pos.end], &mut headers) {
        Ok(Status::Complete((used, headers))) => {
            dbg!(headers);
            dbg!(used);
            let headers = headers
                .iter()
                .map(|header| {
                    // TODO: error propagation
                    let name = HeaderName::from_str(header.name).unwrap();
                    let value =
                        HeaderValue::from_str(std::str::from_utf8(header.value).unwrap()).unwrap();
                    (name, value)
                })
                .collect();
            dbg!(&headers);
            Ok(DecodeResult::Some {
                read: 0,
                buffer,
                new_state: State::TrailerDone(headers),
                new_pos: Position {
                    start: pos.start + used,
                    end: pos.end,
                },
            })
        }
        Ok(Status::Partial) => Ok(DecodeResult::None(buffer)),
        Err(err) => Err(io::Error::new(io::ErrorKind::Other, err.to_string())),
    }
}

impl<R: BufRead + Unpin + Send + 'static> ChunkedDecoder<R> {
    fn poll_read_init(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_inner(cx, buf, decode_init)
    }

    fn poll_read_chunk(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        current: u64,
        len: u64,
    ) -> Poll<io::Result<usize>> {
        self.poll_read_inner(cx, buf, |buffer, pos, buf| {
            decode_chunk(buffer, pos, buf, current, len)
        })
    }

    fn poll_read_chunk_end(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_inner(cx, buf, decode_chunk_end)
    }

    fn poll_read_trailer(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_inner(cx, buf, |buffer, pos, _| decode_trailer(buffer, pos))
    }

    fn poll_read_inner<F>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        mut f: F,
    ) -> Poll<io::Result<usize>>
    where
        F: FnMut(Block<'static>, &Position, &mut [u8]) -> io::Result<DecodeResult>,
    {
        let this = &mut *self;

        let mut n = std::mem::replace(&mut this.current, Position::default());
        let buffer = std::mem::replace(&mut this.buffer, POOL.alloc(INITIAL_CAPACITY));

        let mut buffer = if n.len() > 0 && this.initial_decode {
            match f(buffer, &n, buf)? {
                DecodeResult::Some {
                    read,
                    buffer,
                    new_pos,
                    new_state,
                } => {
                    dbg!(std::str::from_utf8(&buf[..read]));
                    // initial_decode is still true
                    std::mem::replace(&mut this.buffer, buffer);
                    std::mem::replace(&mut this.state, new_state);
                    this.current = new_pos;

                    if State::Done == this.state || read > 0 {
                        return Poll::Ready(Ok(read));
                    }
                    return Poll::Pending;
                }
                DecodeResult::None(buffer) => buffer,
            }
        } else {
            buffer
        };

        loop {
            if n.len() + this.decode_needs >= buffer.capacity() {
                if buffer.capacity() + this.decode_needs < MAX_CAPACITY {
                    buffer.realloc(buffer.capacity() + this.decode_needs);
                } else {
                    std::mem::replace(&mut this.buffer, buffer);
                    this.current = n;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::Other,
                        "incoming data too large",
                    )));
                }
            }

            let bytes_read = match Pin::new(&mut this.inner).poll_read(cx, &mut buffer[n.end..]) {
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

            match f(buffer, &n, buf)? {
                DecodeResult::Some {
                    read,
                    buffer: new_buffer,
                    new_pos,
                    new_state,
                } => {
                    dbg!(read);
                    dbg!(&new_state);
                    dbg!(std::str::from_utf8(&buf[..read]));
                    // current buffer might now contain more data inside, so we need to attempt
                    // to decode it next time
                    this.initial_decode = true;
                    std::mem::replace(&mut this.state, new_state);
                    this.current = new_pos;
                    if State::Done == this.state || read > 0 {
                        std::mem::replace(&mut this.buffer, new_buffer);
                        return Poll::Ready(Ok(read));
                    }

                    buffer = new_buffer;
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
                    } else if n.len() == 0 {
                        // "logical buffer" is empty, there is nothing to decode on the next step
                        this.initial_decode = false;

                        std::mem::replace(&mut this.buffer, buffer);
                        this.current = n;
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "bytes remaining in stream",
                        )));
                    }
                }
            }
        }
    }
}

impl<R: BufRead + Unpin + Send + 'static> Read for ChunkedDecoder<R> {
    #[allow(missing_doc_code_examples)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.state {
            State::Init => {
                // Initial read
                self.poll_read_init(cx, buf)
            }
            State::Chunk(current, len) => {
                // reading a chunk
                self.poll_read_chunk(cx, buf, current, len)
            }
            State::ChunkEnd => self.poll_read_chunk_end(cx, buf),
            State::Trailer => {
                // reading the trailer headers
                self.poll_read_trailer(cx, buf)
            }
            State::TrailerDone(ref mut headers) => {
                dbg!("Done");
                let headers = std::mem::replace(headers, Vec::new());
                for (name, value) in headers.into_iter() {
                    dbg!(&name);
                    let mut fut = self.trailer_sender.send((name, value));
                    async_std::task::ready!(unsafe { Pin::new_unchecked(&mut fut) }.poll(cx));
                }
                self.state = State::Done;
                Poll::Ready(Ok(0))
            }
            State::Done => Poll::Ready(Ok(0)),
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
    const fn new(start: usize, end: usize) -> Position {
        Position { start, end }
    }

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
    },
    None(Block<'static>),
}

#[derive(Debug, PartialEq)]
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
            } => f
                .debug_struct("DecodeResult::Some")
                .field("read", read)
                .field("block", &buffer.len())
                .field("new_pos", new_pos)
                .field("new_state", new_state)
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
                Some((
                    "Expires".parse().unwrap(),
                    "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
                ))
            );
        });
    }
}
