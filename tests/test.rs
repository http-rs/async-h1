use std::error::Error;

#[test]
fn server() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    use async_h1::server;
    use async_std::prelude::*;
    use async_std::{io, net, task};

    task::block_on(async {
        let server = task::spawn(async {
            let listener = net::TcpListener::bind(("localhost", 8080)).await?;
            println!("listening on {}", listener.local_addr()?);
            let mut incoming = listener.incoming();

            while let Some(stream) = incoming.next().await {
                let stream = stream?;
                let (reader, writer) = &mut (&stream, &stream);
                let req = server::decode(reader).await?;
                dbg!(req);
                let body: async_h1::Body<&[u8]> = async_h1::Body::empty();
                let mut res = server::encode(http::Response::new(body)).await?;
                io::copy(&mut res, writer).await?;
            }

            <Result<(), async_h1::Exception>>::Ok(())
        });
        drop(server);
        <Result<(), async_h1::Exception>>::Ok(())
    })
}
