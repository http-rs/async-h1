use async_std::io::Read;

/// Check if the protocol for a stream is HTTP/1.1
pub async fn check(_reader: &mut impl Read) -> bool {
    unimplemented!();
}
