use async_h1::{client, Body};
use async_std::{io, net, task};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let tcp_stream = net::TcpStream::connect("127.0.0.1:8080").await?;
        println!("connecting to {}", tcp_stream.peer_addr()?);

        for i in 0usize..2 {
            let (tcp_reader, tcp_writer) = &mut (&tcp_stream, &tcp_stream);

            println!("making request {}/2", i + 1);

            let body = Body::from_string("hello chashu".to_owned());
            let mut req = client::encode(http::Request::new(body)).await?;
            io::copy(&mut req, tcp_writer).await?;

            // read the response
            let res = client::decode(tcp_reader).await?;
            println!("{:?}", res);
        }
        Ok(())
    })
}
