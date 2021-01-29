use std::fmt;
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_std::io::{self, BufRead, Read};
use futures_core::ready;
use http_types::trailers::{Sender, Trailers};
use pin_project::pin_project;

/// Decodes a chunked body according to
/// https://tools.ietf.org/html/rfc7230#section-4.1
#[pin_project]
#[derive(Debug)]
pub(crate) struct ChunkedDecoder<R: BufRead> {
    /// The underlying stream
    #[pin]
    inner: R,
    /// Current state.
    state: State,
    /// Trailer channel sender.
    trailer_sender: Option<Sender>,
}

impl<R: BufRead> ChunkedDecoder<R> {
    pub(crate) fn new(inner: R, trailer_sender: Sender) -> Self {
        ChunkedDecoder {
            inner,
            state: State::Read(ReadState::BeforeChunk {
                size: 0,
                inner: ChunkSizeState::ChunkSize,
            }),
            trailer_sender: Some(trailer_sender),
        }
    }
}

const MAX_CHUNK_SIZE: u64 = 0x0FFF_FFFF_FFFF_FFFF;

fn read_chunk_size(
    buf: &[u8],
    size: &mut u64,
    state: &mut ChunkSizeState,
) -> io::Result<(usize, bool)> {
    for (offset, c) in buf.iter().copied().enumerate() {
        match *state {
            ChunkSizeState::ChunkSize => match c {
                b'0'..=b'9' => *size = (*size << 4) + (c - b'0') as u64,
                b'a'..=b'f' => *size = (*size << 4) + (c + 10 - b'a') as u64,
                b'A'..=b'F' => *size = (*size << 4) + (c + 10 - b'A') as u64,
                b';' => *state = ChunkSizeState::Extension,
                b'\r' => *state = ChunkSizeState::NewLine,
                _ => return Err(other_err(httparse::InvalidChunkSize)),
            },
            ChunkSizeState::Extension => match c {
                b'\r' => *state = ChunkSizeState::NewLine,
                _ => return Err(other_err(httparse::InvalidChunkSize)),
            },
            ChunkSizeState::NewLine => match c {
                b'\n' => return Ok((offset + 1, true)),
                _ => return Err(other_err(httparse::InvalidChunkSize)),
            },
        }
        if *size > MAX_CHUNK_SIZE {
            return Err(other_err(httparse::InvalidChunkSize));
        }
    }
    Ok((buf.len(), false))
}

impl<R: BufRead> Read for ChunkedDecoder<R> {
    #[allow(missing_doc_code_examples)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let inner_buf = ready!(self.as_mut().poll_fill_buf(cx))?;
        let amt = buf.len().min(inner_buf.len());
        buf[0..amt].copy_from_slice(&inner_buf[0..amt]);
        self.consume(amt);

        Poll::Ready(Ok(amt))
    }
}

impl<R: BufRead> BufRead for ChunkedDecoder<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let mut this = self.project();

        let pass_through_state = loop {
            match this.state {
                State::PassThrough(pass_through_state) => {
                    if pass_through_state.offset < pass_through_state.size {
                        break pass_through_state;
                    } else {
                        *this.state = State::Read(ReadState::AfterChunk { new_line: false });
                    }
                }
                State::Poll(poll_state) => {
                    *this.state = ready!(poll_state.poll(cx, this.trailer_sender))?;
                }
                State::Read(read_state) => {
                    let inner_buf = ready!(this.inner.as_mut().poll_fill_buf(cx))?;

                    if inner_buf.is_empty() {
                        return Poll::Ready(Err(unexpected_eof()));
                    }

                    let mut read = 0;
                    while read < inner_buf.len() {
                        let (nread, next_state) = read_state.advance(&inner_buf[read..])?;
                        read += nread;
                        if let Some(next_state) = next_state {
                            *this.state = next_state;
                            break;
                        }
                    }
                    this.inner.as_mut().consume(read);
                }
                State::Done => return Poll::Ready(Ok(&[])),
            }
        };

        // Unfortunately due to lifetime limitations, this can't be part of the main loop
        let inner_buf = ready!(this.inner.poll_fill_buf(cx))?;

        // Work out how much of the buffer we can pass through
        let max_read = pass_through_state.size - pass_through_state.offset;
        let amt = max_read.min(inner_buf.len() as u64) as usize;

        Poll::Ready(if amt == 0 {
            Err(unexpected_eof())
        } else {
            Ok(&inner_buf[0..amt])
        })
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = self.project();
        if amt > 0 {
            if let State::PassThrough(pass_through_state) = this.state {
                pass_through_state.offset += amt as u64;
                assert!(pass_through_state.offset <= pass_through_state.size);
                this.inner.consume(amt);
            } else {
                panic!("Called consume without first filling buffer");
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ChunkSizeState {
    ChunkSize,
    Extension,
    NewLine,
}

// Decoder state
#[derive(Debug)]
enum State {
    // We're inside a chunk
    PassThrough(PassThroughState),
    // We're reading the framing around a chunk
    Read(ReadState),
    // We're driving an internal future
    Poll(PollState),
    // We're done
    Done,
}

#[derive(Debug)]
struct PassThroughState {
    // Where we are within the chunk
    offset: u64,
    // How big the chunk is
    size: u64,
}

#[derive(Debug)]
enum ReadState {
    // Reading the framing before a chunk
    BeforeChunk { size: u64, inner: ChunkSizeState },
    // Just finished reading the chunk data
    AfterChunk { new_line: bool },
    // Just read CRLF after chunk data
    MaybeTrailer { new_line: bool },
    // Accumulating trailers into a buffer
    Trailer { buffer: Vec<u8> },
}

impl ReadState {
    fn advance(&mut self, buf: &[u8]) -> io::Result<(usize, Option<State>)> {
        match self {
            ReadState::BeforeChunk { size, inner } => {
                let (amt, done) = read_chunk_size(buf, size, inner)?;
                if done {
                    Ok((
                        amt,
                        if *size > 0 {
                            Some(State::PassThrough(PassThroughState {
                                offset: 0,
                                size: *size,
                            }))
                        } else {
                            *self = ReadState::MaybeTrailer { new_line: false };
                            None
                        },
                    ))
                } else {
                    Ok((amt, None))
                }
            }
            ReadState::AfterChunk { new_line } => match (*new_line, buf[0]) {
                (false, b'\r') => {
                    *new_line = true;
                    Ok((1, None))
                }
                (true, b'\n') => {
                    *self = ReadState::BeforeChunk {
                        size: 0,
                        inner: ChunkSizeState::ChunkSize,
                    };
                    Ok((1, None))
                }
                _ => Err(invalid_data_err()),
            },
            ReadState::MaybeTrailer { new_line } => match (*new_line, buf[0]) {
                (false, b'\r') => {
                    *new_line = true;
                    Ok((1, None))
                }
                (true, b'\n') => Ok((
                    1,
                    Some(State::Poll(PollState::TrailerDone(Trailers::new()))),
                )),
                (false, _) => {
                    *self = ReadState::Trailer { buffer: Vec::new() };
                    Ok((0, None))
                }
                (true, _) => Err(invalid_data_err()),
            },
            ReadState::Trailer { buffer } => {
                buffer.extend_from_slice(buf);
                let mut headers = [httparse::EMPTY_HEADER; 16];
                match httparse::parse_headers(&buffer, &mut headers) {
                    Ok(httparse::Status::Complete((amt, headers))) => {
                        let mut trailers = Trailers::new();
                        for header in headers {
                            trailers.insert(
                                header.name,
                                String::from_utf8_lossy(header.value).as_ref(),
                            );
                        }
                        Ok((amt, Some(State::Poll(PollState::TrailerDone(trailers)))))
                    }
                    Ok(httparse::Status::Partial) => Ok((buf.len(), None)),
                    Err(err) => Err(other_err(err)),
                }
            }
        }
    }
}

enum PollState {
    /// Trailers were decoded, are now set to the decoded trailers.
    TrailerDone(Trailers),
    TrailerSending(Pin<Box<dyn Future<Output = ()> + 'static + Send + Sync>>),
}

impl PollState {
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        trailer_sender: &mut Option<Sender>,
    ) -> Poll<io::Result<State>> {
        Poll::Ready(match self {
            PollState::TrailerDone(trailers) => {
                let trailers = std::mem::replace(trailers, Trailers::new());
                let sender = trailer_sender
                    .take()
                    .expect("invalid chunked state, tried sending multiple trailers");
                let fut = Box::pin(sender.send(trailers));
                Ok(State::Poll(PollState::TrailerSending(fut)))
            }
            PollState::TrailerSending(fut) => {
                ready!(fut.as_mut().poll(cx));
                Ok(State::Done)
            }
        })
    }
}

impl fmt::Debug for PollState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PollState {{ .. }}")
    }
}

fn other_err<E: Display>(err: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err.to_string())
}

fn invalid_data_err() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn unexpected_eof() -> io::Error {
    io::Error::from(io::ErrorKind::UnexpectedEof)
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

            let (s, _r) = async_channel::bounded(1);
            let sender = Sender::new(s);
            let mut decoder = ChunkedDecoder::new(input, sender);

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
    fn test_chunked_big() {
        async_std::task::block_on(async move {
            let mut input: Vec<u8> = b"800\r\n".to_vec();
            input.extend(vec![b'X'; 2048]);
            input.extend(b"\r\n1800\r\n");
            input.extend(vec![b'Y'; 6144]);
            input.extend(b"\r\n800\r\n");
            input.extend(vec![b'Z'; 2048]);
            input.extend(b"\r\n0\r\n\r\n");

            let (s, _r) = async_channel::bounded(1);
            let sender = Sender::new(s);
            let mut decoder = ChunkedDecoder::new(async_std::io::Cursor::new(input), sender);

            let mut output = String::new();
            decoder.read_to_string(&mut output).await.unwrap();

            let mut expected = vec![b'X'; 2048];
            expected.extend(vec![b'Y'; 6144]);
            expected.extend(vec![b'Z'; 2048]);
            assert_eq!(output.len(), 10240);
            assert_eq!(output.as_bytes(), expected.as_slice());
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
            let (s, r) = async_channel::bounded(1);
            let sender = Sender::new(s);
            let mut decoder = ChunkedDecoder::new(input, sender);

            let mut output = String::new();
            decoder.read_to_string(&mut output).await.unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork");

            let trailers = r.recv().await.unwrap();
            assert_eq!(trailers.iter().count(), 1);
            assert_eq!(trailers["Expires"], "Wed, 21 Oct 2015 07:28:00 GMT");
        });
    }
}
