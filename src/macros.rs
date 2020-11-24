macro_rules! read_to_end {
    ($expr:expr) => {
        match $expr {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(0)) => (),
            Poll::Ready(Ok(n)) => return Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
        }
    };
}
