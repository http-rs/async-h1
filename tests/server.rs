mod common;
use async_h1::server;
use async_std::io::Cursor;
use async_std::task;
use common::read_fixture;
use http_types::{Response, StatusCode};

#[test]
fn test_basic_request() {
    let request = read_fixture("request1");
    let expected = read_fixture("response1");
    let mut actual = Vec::new();
    let addr = "http://example.com";

    assert!(
        actual,
        expected,
        server::accept(addr, Cursor::new(request), &mut actual, |_req| {
            async {
                let mut resp = Response::new(StatusCode::Ok);
                resp.set_body("");
                Ok(resp)
            }
        })
    );
}
