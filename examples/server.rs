use async_h1::server;
use async_std::prelude::*;
use async_std::{net, task, io};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("localhost", 8080)).await?;
        println!("listening on {}", listener.local_addr()?);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            let (reader, writer) = &mut (&stream, &stream);
            let req = server::decode(reader).await?;
            dbg!(req);
            let res = http::Response::new(async_h1::Body {});
            let mut res = server::encode(res).await?;
            io::copy(&mut res, writer).await?;
        }

        Ok(())
    })
}
