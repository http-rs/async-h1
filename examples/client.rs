use async_h1::{client, Body};
use async_std::{io, net, task};
use http::Response;

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let stream = net::TcpStream::connect("127.0.0.1:8080").await?;
        let (reader, writer) = &mut (&stream, &stream);
        let body = Body::from_string("hello chashu".to_owned());
        let mut req = client::encode(http::Request::new(body)).await?;
        io::copy(&mut req, writer).await?;
        let res: Response<Body<io::BufReader<&mut &async_std::net::TcpStream>>> = client::decode(reader).await?;
        println!("Response {:?}", res);
        Ok::<(), async_h1::Exception>(())
    })
}
