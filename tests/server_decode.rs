mod test_utils;
mod server_decode {
    use super::test_utils::TestIO;
    use async_h1::ServerOptions;
    use async_std::io::prelude::*;
    use http_types::Request;
    use http_types::Result;
    use http_types::Url;
    use http_types::{headers::TRANSFER_ENCODING, Version};
    use pretty_assertions::assert_eq;

    async fn decode_lines(lines: Vec<&str>, options: ServerOptions) -> Result<Option<Request>> {
        let s = lines.join("\r\n");
        let (mut client, server) = TestIO::new();
        client.write_all(s.as_bytes()).await?;
        client.close();
        async_h1::server::decode(server, &options)
            .await
            .map(|r| r.map(|(r, _)| r))
    }

    async fn decode_lines_default(lines: Vec<&str>) -> Result<Option<Request>> {
        decode_lines(lines, ServerOptions::default()).await
    }

    #[async_std::test]
    async fn post_with_body() -> Result<()> {
        let mut request = decode_lines_default(vec![
            "POST / HTTP/1.1",
            "host: localhost:8080",
            "content-length: 5",
            "content-type: text/plain;charset=utf-8",
            "another-header: header value",
            "another-header: other header value",
            "",
            "hello",
            "",
        ])
        .await?
        .unwrap();

        assert_eq!(request.method(), http_types::Method::Post);
        assert_eq!(request.body_string().await?, "hello");
        assert_eq!(request.content_type(), Some(http_types::mime::PLAIN));
        assert_eq!(request.version(), Some(http_types::Version::Http1_1));
        assert_eq!(request.host(), Some("localhost:8080"));
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
        let mut request = decode_lines_default(vec![
            "POST / HTTP/1.1",
            "host: localhost:8080",
            "transfer-encoding: chunked",
            "content-type: text/plain;charset=utf-8",
            "",
            "1",
            "h",
            "1",
            "e",
            "3",
            "llo",
            "0",
            "",
        ])
        .await?
        .unwrap();

        assert_eq!(request[TRANSFER_ENCODING], "chunked");
        assert_eq!(request.body_string().await?, "hello");

        Ok(())
    }

    #[ignore = r#"
       the test previously did not actually assert the correct thing prevously
       and the behavior does not yet work as intended
    "#]
    #[async_std::test]
    async fn invalid_trailer() -> Result<()> {
        let mut request = decode_lines_default(vec![
            "GET / HTTP/1.1",
            "host: domain.com",
            "content-type: application/octet-stream",
            "transfer-encoding: chunked",
            "trailer: x-invalid",
            "",
            "0",
            "x-invalid: å",
            "",
        ])
        .await?
        .unwrap();

        assert!(request.body_string().await.is_err());

        Ok(())
    }

    #[async_std::test]
    async fn unexpected_eof() -> Result<()> {
        let mut request = decode_lines_default(vec![
            "POST / HTTP/1.1",
            "host: example.com",
            "content-type: text/plain",
            "content-length: 11",
            "",
            "not 11",
        ])
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

    #[async_std::test]
    async fn http_1_0_without_host_header() -> Result<()> {
        let request = decode_lines(
            vec!["GET /path?query#fragment HTTP/1.0", "", ""],
            ServerOptions::new().with_default_host("website.com"),
        )
        .await?
        .unwrap();

        assert_eq!(request.version(), Some(Version::Http1_0));
        assert_eq!(
            request.url().to_string(),
            "http://website.com/path?query#fragment"
        );
        Ok(())
    }

    #[async_std::test]
    async fn http_1_1_without_host_header() -> Result<()> {
        let result = decode_lines(
            vec!["GET /path?query#fragment HTTP/1.1", "", ""],
            ServerOptions::default(),
        )
        .await;

        assert!(result.is_err());

        Ok(())
    }

    #[async_std::test]
    async fn http_1_0_with_host_header() -> Result<()> {
        let request = decode_lines(
            vec![
                "GET /path?query#fragment HTTP/1.0",
                "host: example.com",
                "",
                "",
            ],
            ServerOptions::new().with_default_host("website.com"),
        )
        .await?
        .unwrap();

        assert_eq!(request.version(), Some(Version::Http1_0));
        assert_eq!(
            request.url().to_string(),
            "http://example.com/path?query#fragment"
        );
        Ok(())
    }

    #[async_std::test]
    async fn http_1_0_request_with_no_default_host_is_provided() -> Result<()> {
        let request = decode_lines(
            vec!["GET /path?query#fragment HTTP/1.0", "", ""],
            ServerOptions::default(),
        )
        .await;

        assert!(request.is_err());
        Ok(())
    }

    #[async_std::test]
    async fn http_1_0_request_with_no_default_host_is_provided_even_if_host_header_exists(
    ) -> Result<()> {
        let result = decode_lines(
            vec![
                "GET /path?query#fragment HTTP/1.0",
                "host: example.com",
                "",
                "",
            ],
            ServerOptions::default(),
        )
        .await;

        assert!(result.is_err());
        Ok(())
    }
}
