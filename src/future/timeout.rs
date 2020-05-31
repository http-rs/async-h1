use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use smol::Timer;

use pin_project_lite::pin_project;
use futures_core::task::{Context, Poll};

/// missing docs
pub(crate) async fn timeout<F, T>(dur: Duration, f: F) -> Result<T, TimeoutError>
    where
        F: Future<Output = T>,
{
    TimeoutFuture::new(f, dur).await
}

pin_project! {
    /// A future that times out after a duration of time.
    pub struct TimeoutFuture<F> {
        #[pin]
        future: F,
        #[pin]
        delay: Timer,
    }
}

impl<F> TimeoutFuture<F> {
    #[allow(dead_code)]
    pub(super) fn new(future: F, dur: Duration) -> TimeoutFuture<F> {
        TimeoutFuture {
            future,
            delay: smol::Timer::after(dur),
        }
    }
}

impl<F: Future> Future for TimeoutFuture<F> {
    type Output = Result<F::Output, TimeoutError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        match this.future.poll(cx) {
            Poll::Ready(v) => Poll::Ready(Ok(v)),
            Poll::Pending => match this.delay.poll(cx) {
                Poll::Ready(_) => Poll::Ready(Err(TimeoutError { _private: () })),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

/// An error returned when a future times out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeoutError {
    _private: (),
}

impl Error for TimeoutError {}

impl fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "future has timed out".fmt(f)
    }
}