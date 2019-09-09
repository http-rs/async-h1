use async_h1::{server, Body};
use async_std::prelude::*;
use async_std::{net, task, io};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("127.0.0.1", 8080)).await?;
        println!("listening on {}", listener.local_addr()?);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            let (reader, writer) = &mut (&stream, &stream);
            let req = server::decode(reader).await?;
            // dbg!(req);
            let body = Body::from_string("hello chashu".to_owned());
            let mut res = server::encode(http::Response::new(body)).await?;
            io::copy(&mut res, writer).await?;
        }

        Ok(())
    })
}
