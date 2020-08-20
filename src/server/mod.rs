//! Process HTTP connections on the server.

use std::time::Duration;

use log::{trace, error};

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self};
use async_std::io::{Read, Write};
use async_std::io::{ReadExt};
use http_types::{Request, Response};

mod decode;
mod encode;
mod channel_bufread;

pub use decode::decode;
use decode::decode_with_bufread;
pub use encode::{ Encoder, Options as EncoderOptions };

use async_std::sync::Arc;
use futures_channel::oneshot;
use futures_util::select;
use futures_util::future::FutureExt;

/// Configure the server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Timeout to handle headers. Defaults to 60s.
    headers_timeout: Option<Duration>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            headers_timeout: Some(Duration::from_secs(60)),
        }
    }
}

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept<RW, F, Fut>(io: RW, endpoint: F) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    accept_with_opts(io, endpoint, Default::default()).await
}

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default (actually doesn't).
pub async fn accept_with_opts<RW, F, Fut>(
    io: RW,
    endpoint: F,
    opts: ServerOptions,
) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    let mut out_io = io.clone();
    
    loop {
        let mut _io = io.clone();
        // io for headers and normal body reading
        let (io_primary, mut io_primary_send) = channel_bufread::BufReader::with_capacity(8 * 1024, io.clone());
        // io used with Body::io
        let (io_upgrade, mut io_upgrade_send) = channel_bufread::BufReader::with_capacity(8 * 1024, io.clone());
        
        let (io_complete_send, io_complete_recv) = oneshot::channel::<()>();
        
        use futures_util::sink::SinkExt;
        async_std::task::spawn(async move {
            let mut io_end_fut = io_complete_recv.fuse();
            
            let (mut body_start, mut n) = {
                let mut buf: Vec<u8> = Vec::new();
                buf.resize(8 * 1024, 0);
                let buf_len = buf.len();
                let mut pos = 0;
                // read up to 8kb of headers, the implementation already limits
                // headers to 8kb
                let header_end_pos = 'a: loop {
                    let n = match _io.read(&mut buf[pos..buf_len]).await {
                        Ok(0) => {
                            let res = Ok((Arc::new(vec![].into_boxed_slice()), 0usize));
                            match io_primary_send.send(res).await {
                                Ok(_) => (),
                                Err(err) if err.is_disconnected() => (),
                                Err(err) => unreachable!("Unexpected channel err {}", err),
                            }
                            
                            return;
                        }
                        Ok(n) => {
                            pos += n;
                            n
                        }
                        Err(err) => {
                            match io_primary_send.send(Err(err)).await {
                                Ok(_) => (),
                                Err(err) if err.is_disconnected() => (),
                                Err(err) => unreachable!("Unexpected channel err {}", err),
                            }
                            return
                        }
                    };
                    
                    if pos < 3 {
                        continue
                    }
                    
                    for idx in 3..n {
                        if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
                            break 'a idx;
                        }
                    }
                    
                    if pos == buf.len() {
                        let res = Ok((Arc::new(vec![].into_boxed_slice()), 0usize));
                        match io_primary_send.send(res).await {
                            Ok(_) => (),
                            Err(err) if err.is_disconnected() => (),
                            Err(err) => unreachable!("Unexpected channel err {}", err),
                        }
                        error!("Headers exceeded 8kb, rejecting request");
                        return
                    }
                };
                
                let body_start = buf.split_off(header_end_pos+1);
                
                let res = Ok((Arc::new(buf.into_boxed_slice()), header_end_pos+1));
                match io_primary_send.send(res).await {
                    Ok(_) => (),
                    Err(err) if err.is_disconnected() => return,
                    Err(err) => unreachable!("Unexpected channel err {}", err),
                }
                
                let body_start_n = pos - (header_end_pos+1);
                
                (body_start, body_start_n)
            };
            
            let mut read_failure = None;
            
            // read more if no body has been recieved
            if n == 0 {
                // use a smaller buffer than used to read headers
                // to avoid resize in most cases
                if body_start.len() < 6 * 1024 {
                    body_start.resize(8 * 1024, 0);
                }
                
                n = match _io.read(&mut body_start).await {
                    Ok(n) => {
                        n
                    }
                    Err(err) => {
                        read_failure = Some(err);
                        0
                    }
                };
            }
            
            let body_start = Arc::new(body_start.into_boxed_slice());
            
            let is_read_failure = read_failure.is_some();
            let (res_2, res_3) = match read_failure {
                Some(err) => {
                    let error_kind = err.kind();
                    (
                        Err(err),
                        Err(io::Error::new(error_kind, "First body read failed")),
                    )
                }
                None => (
                    Ok((body_start.clone(), n)),
                    Ok((body_start.clone(), n)),
                ),
            };
            
            let mut io_primary_fut = io_primary_send.send(res_2).fuse();
            let mut io_upgrade_fut = io_upgrade_send.send(res_3).fuse();
            let (is_io_body, unfinished_send) = select! {
                res = io_primary_fut => {
                    match res {
                        Ok(()) => (false, None),
                        Err(err) if err.is_disconnected() => (true, Some(io_upgrade_fut)),
                        // the channel should never be full without try_send
                        Err(err) => unreachable!("Unexpected channel err {}", err),
                    }
                }
                res = io_upgrade_fut => {
                    match res {
                        Ok(()) => (true, None),
                        Err(err) if err.is_disconnected() => (false, Some(io_primary_fut)),
                        Err(err) => unreachable!("Unexpected channel err {}", err),
                    }
                }
                _ = io_end_fut => {
                    return
                }
            };
            
            if is_read_failure {
                return
            }
            
            if let Some(fut) = unfinished_send {
                match fut.await {
                    Ok(()) => (),
                    Err(err) if err.is_disconnected() => return,
                    Err(err) => unreachable!("Unexpected channel err {}", err),
                };
            }
            
            trace!("Reader reading to Body::io {}", is_io_body);
            let mut io_send = if is_io_body {
                io_upgrade_send
            } else {
                io_primary_send
            };
            
            let mut buf = Vec::new();
            buf.resize(8 * 1024, 0);
            let buf = buf.into_boxed_slice();
            let mut buf_ready = Arc::new(buf);
            
            let mut buf = Vec::new();
            buf.resize(8 * 1024, 0);
            let buf = buf.into_boxed_slice();
            let mut buf_pending = Arc::new(buf);
            
            let mut read_fut = _io.read(Arc::get_mut(&mut buf_pending).unwrap()).fuse();
            
            loop {
                let res = select! {
                    res = read_fut => {
                        std::mem::swap(&mut buf_pending, &mut buf_ready);
                        
                        read_fut = _io.read(Arc::get_mut(&mut buf_pending).unwrap()).fuse();
                        res
                    }
                    _ = io_end_fut => {
                        break
                    }
                };
                
                n = match res {
                    Ok(n) => {
                        n
                    }
                    Err(err) => {
                        match io_send.send(Err(err)).await {
                            Ok(_) => (),
                            Err(err) if err.is_disconnected() => (),
                            Err(err) => unreachable!("Unexpected channel err {}", err),
                        }
                        
                        return
                    }
                };
                
                match io_send.send(Ok((buf_ready.clone(), n))).await {
                    Ok(_) => (),
                    Err(err) if err.is_disconnected() => {
                        break
                    }
                    Err(err) => unreachable!("Unexpected channel err {}", err),
                }
                
                if n == 0 {
                    break
                }
            }
            
            return;
        });
        
        // Decode a new request, timing out if this takes longer than the timeout duration.
        let fut = decode_with_bufread(io_primary);

        let (mut req, _body_type) = if let Some(timeout_duration) = opts.headers_timeout {
            match timeout(timeout_duration, fut).await {
                Ok(Ok(Some(r))) => r,
                Ok(Ok(None)) | Err(TimeoutError { .. }) => break, /* EOF or timeout */
                Ok(Err(e)) => return Err(e),
            }
        } else {
            match fut.await? {
                Some(r) => r,
                None => break, /* EOF */
            }
        };

        req.set_version(Some(http_types::Version::Http1_1));
        
        let method = req.method();
        // Pass the request to the endpoint and encode the response.
        let mut res = endpoint(req).await?;

        let io_handler = res.take_io();

        let mut encoder_options = EncoderOptions::default();
        if let Some(_) = io_handler {
            encoder_options.disable_chunked_encoding = true;
        }

        let mut encoder = Encoder::new_with_options(res, method, encoder_options);

        // Stream the response to the writer.
        io::copy(&mut encoder, &mut out_io).await?;
        
        if let Some(upgrade) = io_handler {
            upgrade.call(Box::new(io_upgrade)).await;
            
            let _ = io_complete_send.send(());
            return Ok(())
        } else {
            let _ = io_complete_send.send(());
        }
        
        // avoid unexpected behaviour since keep alive isn't actually implemented
        break
    }

    Ok(())
}
