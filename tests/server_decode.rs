mod server_decode {
    use async_dup::{Arc, Mutex};
    use async_std::io::{Cursor, ReadExt};
    use duplexify::Duplex;
    use http_types::Request;
    use http_types::Result;
    use http_types::Url;
    use pretty_assertions::assert_eq;

    async fn decode_str(s: &'static str) -> Result<Option<Request>> {
        async_h1::server::decode(Duplex::new(
            Arc::new(Mutex::new(Cursor::new(s.replace("\n", "\r\n")))),
            Arc::new(Mutex::new(Cursor::new(vec![]))),
        ))
        .await
    }

    #[async_std::test]
    async fn post_with_body() -> Result<()> {
        let mut request = decode_str(
            r#"POST / HTTP/1.1
host: localhost:8080
content-length: 5
content-type: text/plain;charset=utf-8
another-header: header value
another-header: other header value

hello

"#,
        )
        .await?
        .unwrap();

        assert_eq!(request.method(), http_types::Method::Post);
        assert_eq!(request.body_string().await?, "hello");
        assert_eq!(request.content_type(), Some(http_types::mime::PLAIN));
        assert_eq!(request.version(), Some(http_types::Version::Http1_1));
        assert_eq!(
            request.url(),
            &Url::parse("http://localhost:8080/").unwrap()
        );

        let custom_header = request.header("another-header").unwrap();
        assert_eq!(custom_header[0], "header value");
        assert_eq!(custom_header[1], "other header value");

        Ok(())
    }

    #[async_std::test]
    async fn chunked() -> Result<()> {
        let mut request = decode_str(
            r#"POST / HTTP/1.1
host: localhost:8080
transfer-encoding: chunked
content-type: text/plain;charset=utf-8

1
h
1
e
3
llo
0

"#,
        )
        .await?
        .unwrap();

        assert_eq!(request.body_string().await?, "hello");

        Ok(())
    }

    #[ignore = r#"
       the test previously did not actually assert the correct thing prevously
       and the behavior does not yet work as intended
    "#]
    #[async_std::test]
    async fn invalid_trailer() -> Result<()> {
        let mut request = decode_str(
            r#"GET / HTTP/1.1
host: domain.com
content-type: application/octet-stream
transfer-encoding: chunked
trailer: x-invalid

0
x-invalid: å

"#,
        )
        .await?
        .unwrap();

        assert!(request.body_string().await.is_err());

        Ok(())
    }

    #[async_std::test]
    async fn unexpected_eof() -> Result<()> {
        let mut request = decode_str(
            r#"POST / HTTP/1.1
host: example.com
content-type: text/plain
content-length: 11

not 11"#,
        )
        .await?
        .unwrap();

        let mut string = String::new();
        // we use read_to_string because although not currently the
        // case, at some point soon body_string will error if the
        // retrieved content length is not the same as the header (if
        // the client disconnects)
        request.read_to_string(&mut string).await?;
        assert_eq!(string, "not 11");

        Ok(())
    }
}
