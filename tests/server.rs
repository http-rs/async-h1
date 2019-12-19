mod common;
use async_h1::server;
use async_std::task;
use common::read_fixture;
use http_types::{Response, StatusCode};

#[test]
fn test_basic_request() {
    task::block_on(async {
        let io = read_fixture("request1").await;
        let mut expected = read_fixture("response1").await;
        let addr = "http://example.com";

        assert!(
            io.clone(),
            expected,
            server::accept(addr, io.clone(), |_req| {
                async {
                    let mut resp = Response::new(StatusCode::Ok);
                    resp.set_body("");
                    Ok(resp)
                }
            })
        );
    })
}
