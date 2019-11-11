use async_h1::{server, Body};
use async_std::prelude::*;
use async_std::{io, net, task};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("127.0.0.1", 8080)).await?;
        println!("listening on {}", listener.local_addr()?);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            // task::spawn(async move {
            //     let stream = stream?;
            //     let (reader, writer) = &mut (&stream, &stream);
            //     let req = server::decode(reader).await?;
            //     if let server::OptionalRequest::Request(req) = req {
            //         // dbg!(req);
            //         let body = Body::from_string("hello chashu".to_owned());
            //         let mut res = server::encode(http::Response::new(body)).await?;
            //         io::copy(&mut res, writer).await?;
            //     }
            //     Ok::<(), async_h1::Exception>(())
            // });
            task::spawn(async {
                let stream = stream?;
                let (reader, writer) = &mut (&stream, &stream);
                server::connect(reader, writer, |req| {
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
