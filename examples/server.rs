use async_h1::{server, Body};
use async_std::net;
use async_std::prelude::*;
use async_std::task;

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("127.0.0.1", 8080)).await?;
        println!("listening on {}", listener.local_addr()?);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            task::spawn(async {
                let stream = stream?;
                let (reader, writer) = &mut (&stream, &stream);
                server::connect(reader, writer, |_| {
                    async {
                        let body = Body::from_string("hello chashu".to_owned());
                        Ok(http::Response::new(body))
                    }
                })
                .await
            });
        }
        Ok(())
    })
}
