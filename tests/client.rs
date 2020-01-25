use crate::common::{fixture_path, munge_date};
use async_h1::client;
use async_std::fs::File;
use async_std::io::SeekFrom;
use async_std::prelude::*;
use http_types::{headers, Method, Request};
use url::Url;

mod common;

#[async_std::test]
async fn test_encode_request_add_date() {
    let mut request_fixture = File::open(fixture_path("fixtures/client-request1.txt"))
        .await
        .unwrap();
    let mut expected = String::new();
    request_fixture.read_to_string(&mut expected).await.unwrap();

    let url = Url::parse("http://example.com").unwrap();
    let req = Request::new(Method::Get, url);

    let mut encoded_req = client::encode(req).await.unwrap();

    let mut actual = String::new();
    encoded_req.read_to_string(&mut actual).await.unwrap();

    munge_date(&mut expected, &mut actual);

    pretty_assertions::assert_eq!(actual, expected);
}

#[async_std::test]
async fn test_response_no_date() {
    let mut response_fixture = File::open(fixture_path("fixtures/response-no-date.txt"))
        .await
        .unwrap();
    response_fixture.seek(SeekFrom::Start(0)).await.unwrap();

    let res = client::decode(response_fixture).await.unwrap();

    pretty_assertions::assert_eq!(res.header(&headers::DATE).is_some(), true);
}
