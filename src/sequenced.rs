use core::future::Future;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll};

use futures_channel::oneshot;
use futures_core::ready;
use futures_io::{AsyncBufRead, AsyncRead, AsyncWrite};
use futures_lite::future::poll_fn;

#[derive(Debug)]
enum SequencedState<T> {
    Active {
        value: T,
        poisoned: bool,
    },
    Waiting {
        receiver: oneshot::Receiver<Self>,
        poisoned: Option<bool>,
    },
}

/// Allows multiple asynchronous tasks to access the same reader or writer concurrently
/// without conflicting.
/// The `split_seq` and `split_seq_rev` methods produce a new instance of the type such that
/// all I/O operations on one instance are sequenced before all I/O operations on the other.
///
/// When one task has finished with the reader/writer it should call `release`, which will
/// unblock operations on the task with the other instance. If dropped without calling
/// `release`, the inner reader/writer will become poisoned before being returned. The
/// caller can explicitly remove the poisoned status.
///
/// The `Sequenced<T>` can be split as many times as necessary, and it is valid to call
/// `release()` at any time, although no further operations can be performed via a released
/// instance. If this type is dropped without calling `release()`, then the reader/writer will
/// become poisoned.
///
/// As only one task has access to the reader/writer at once, no additional synchronization
/// is necessary, and so this wrapper adds very little overhead. What synchronization does
/// occur only needs to happen when an instance is released, in order to send its state to
/// the next instance in the sequence.
///
/// Merging can be achieved by simply releasing one of the two instances, and then using the
/// other one as normal. It does not matter Which one is released.
#[derive(Debug)]
pub struct Sequenced<T> {
    parent: Option<oneshot::Sender<SequencedState<T>>>,
    state: Option<SequencedState<T>>,
}

impl<T> Sequenced<T> {
    /// Constructs a new sequenced reader/writer
    pub fn new(value: T) -> Self {
        Self {
            parent: None,
            state: Some(SequencedState::Active {
                value,
                poisoned: false,
            }),
        }
    }
    /// Splits this reader/writer into two such that the returned instance is sequenced before this one.
    pub fn split_seq(&mut self) -> Self {
        let (sender, receiver) = oneshot::channel();
        let state = mem::replace(
            &mut self.state,
            Some(SequencedState::Waiting {
                receiver,
                poisoned: None,
            }),
        );
        Self {
            parent: Some(sender),
            state,
        }
    }
    /// Splits this reader/writer into two such that the returned instance is sequenced after this one.
    pub fn split_seq_rev(&mut self) -> Self {
        let other = self.split_seq();
        mem::replace(self, other)
    }

    /// Release this reader/writer immediately, allowing instances sequenced after this one to proceed.
    pub fn release(&mut self) {
        if let (Some(state), Some(parent)) = (self.state.take(), self.parent.take()) {
            let _ = parent.send(state);
        }
    }
    fn set_poisoned(&mut self, value: bool) {
        match &mut self.state {
            Some(SequencedState::Active { poisoned, .. }) => *poisoned = value,
            Some(SequencedState::Waiting { poisoned, .. }) => *poisoned = Some(value),
            None => {}
        }
    }
    /// Removes the poison status if set
    pub(crate) fn cure(&mut self) {
        self.set_poisoned(false)
    }
    fn resolve(&mut self, cx: &mut Context<'_>) -> Poll<Option<&mut T>> {
        while let Some(SequencedState::Waiting { receiver, poisoned }) = &mut self.state {
            if let Some(sender) = &self.parent {
                // Check if we're waiting on ourselves.
                if sender.is_connected_to(receiver) {
                    return Poll::Ready(None);
                }
            }
            let poisoned = *poisoned;
            self.state = ready!(Pin::new(receiver).poll(cx)).ok();
            if let Some(value) = poisoned {
                self.set_poisoned(value)
            }
        }
        Poll::Ready(match &mut self.state {
            Some(SequencedState::Active {
                poisoned: false,
                value,
            }) => Some(value),
            Some(SequencedState::Active { poisoned: true, .. }) => None,
            Some(SequencedState::Waiting { .. }) => unreachable!(),
            None => None,
        })
    }
    /// Attempt to take the inner reader/writer. This will require waiting until prior instances
    /// have been released, and will fail with `None` if any were dropped without being released,
    /// or were themselves taken.
    /// Instances sequenced after this one will see the reader/writer be closed.
    pub fn poll_take_inner(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        ready!(self.as_mut().resolve(cx));
        if let Some(SequencedState::Active {
            value,
            poisoned: false,
        }) = self.as_mut().state.take()
        {
            Poll::Ready(Some(value))
        } else {
            Poll::Ready(None)
        }
    }
    /// Attempt to take the inner reader/writer. This will require waiting until prior instances
    /// have been released, and will fail with `None` if any were dropped without being released,
    /// or were themselves taken.
    /// Instances sequenced after this one will see the reader/writer be closed.
    pub async fn take_inner(&mut self) -> Option<T> {
        poll_fn(|cx| Pin::new(&mut *self).poll_take_inner(cx)).await
    }

    /// Swap the two reader/writers at this sequence point.
    pub fn swap(&mut self, other: &mut Self) {
        mem::swap(&mut self.state, &mut other.state);
    }
}

impl<T> Drop for Sequenced<T> {
    // Poison and release the inner reader/writer. Has no effect if the reader/writer
    // was already released.
    fn drop(&mut self) {
        self.set_poisoned(true);
        self.release();
    }
}

impl<T> Unpin for Sequenced<T> {}

impl<T: Unpin + AsyncRead> AsyncRead for Sequenced<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<futures_io::Result<usize>> {
        if let Some(inner) = ready!(self.get_mut().resolve(cx)) {
            Pin::new(inner).poll_read(cx, buf)
        } else {
            Poll::Ready(Ok(0))
        }
    }
}

impl<T: Unpin + AsyncBufRead> AsyncBufRead for Sequenced<T> {
    fn poll_fill_buf(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<futures_io::Result<&[u8]>> {
        if let Some(inner) = ready!(self.get_mut().resolve(cx)) {
            Pin::new(inner).poll_fill_buf(cx)
        } else {
            Poll::Ready(Ok(&[]))
        }
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        if let Some(SequencedState::Active {
            value,
            poisoned: false,
        }) = &mut self.get_mut().state
        {
            Pin::new(value).consume(amt);
        } else if amt > 0 {
            panic!("Called `consume()` without having filled the buffer")
        }
    }
}

impl<T: Unpin + AsyncWrite> AsyncWrite for Sequenced<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<futures_io::Result<usize>> {
        if let Some(inner) = ready!(self.get_mut().resolve(cx)) {
            Pin::new(inner).poll_write(cx, buf)
        } else {
            Poll::Ready(Ok(0))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        if let Some(inner) = ready!(self.get_mut().resolve(cx)) {
            Pin::new(inner).poll_flush(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        if let Some(inner) = ready!(self.get_mut().resolve(cx)) {
            Pin::new(inner).poll_close(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    }
}
