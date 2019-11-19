mod common;
use async_h1::{server, Body};
use async_std::task;
use common::read_fixture;
use http::Response;

#[test]
fn test_basic_request() {
    let request = read_fixture("request1");
    let expected = read_fixture("response1");
    let mut actual = Vec::new();

    assert!(
        actual,
        expected,
        server::connect(&request[..], &mut actual, |_req| {
            async { Ok(Response::new(Body::empty("".as_bytes()))) }
        })
    );
}
