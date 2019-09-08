use futures_io::AsyncRead;

/// Check if the protocol for a stream is HTTP/1.1
pub async fn check(_reader: &mut impl AsyncRead) -> bool {
    unimplemented!();
}
