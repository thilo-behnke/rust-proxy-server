pub mod proxy_server {
    use std::convert::Infallible;
    use std::fmt::Debug;
    use std::time::{SystemTime, UNIX_EPOCH};
    use hyper::{Client, Server, Body, Method, Request, Response};
    use hyper::server::conn::AddrStream;
    use hyper::service::{make_service_fn, service_fn};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::task::JoinHandle;

    const BUF_SIZE: usize = 1024;
    const TIMEOUT_THRESHOLD: usize = 10;

    pub struct ProxyServer {}

    impl ProxyServer {
        pub fn create() -> ProxyServer {
            ProxyServer {}
        }

        pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let make_svc = make_service_fn(|_: &AddrStream| {
                async {
                    Ok::<_, Infallible>(
                        service_fn(|req: Request<Body>| {
                            return handle_request(req);
                        })
                    )
                }
            });

            let addr = ([127, 0, 0, 1], 3000).into();

            let server = Server::bind(&addr).serve(make_svc);

            println!("Listening on http://{}", addr);

            server.await?;

            Ok(())
        }
    }

    async fn handle_request<'a>(
        mut req: Request<Body>
    ) -> Result<Response<Body>, hyper::Error> {
        let client = Client::new();
        println!("Request: {:?}", req);
        let res = match req.method() {
            &Method::GET => Ok(client.get(req.uri().clone()).await?),
            &Method::CONNECT => {
                let res = Response::new(Body::empty());

                tokio::task::spawn(async move {
                    let uri = req.uri().to_string();
                    println!("[{}] Trying to use existing connection to client...", uri);
                    match (hyper::upgrade::on(&mut req).await, TcpStream::connect(&uri).await) {
                        (Ok(upgraded), Ok(host_stream)) => {
                            let (client_reader, client_writer) = tokio::io::split(upgraded);
                            println!("[{}] Client connection ready: {:?} / {:?}", uri, client_reader, client_writer);
                            let (host_reader, host_writer) = tokio::io::split(host_stream);
                            println!("[{}] Host connection ready: {:?} / {:?}", uri, host_reader, host_writer);

                            let client_handle = connect(format!("client -> host ({})", uri), client_reader, host_writer).await;
                            let host_handle = connect(format!("host ({}) -> client", uri), host_reader, client_writer).await;

                            tokio::join!(client_handle);
                            println!("[{}] Client is finished, closing host connection", uri);
                            host_handle.abort();
                            println!("[{}] connections are closed.", uri);
                        }
                        _ => eprintln!("failed to setup tunnel"),
                    }
                });

                println!("Handler set up, sending 200 to commence tunnel.");
                Ok(res)
            }
            m => panic!("Method not supported: {:?}", m),
        };
        println!("Response: {:?}", res);
        res
    }

    async fn connect(name: String, reader: impl AsyncRead + Debug + Send + 'static, writer: impl AsyncWrite + Debug + Send + 'static) -> JoinHandle<()> {
        println!("[{}] Setting up connection between reader {:?} and writer {:?}", name, reader, writer);
        return tokio::task::spawn(async move {
            let mut pinned_reader = Box::pin(reader);
            let mut pinned_writer = Box::pin(writer);

            let mut last_read: u64 = 0;
            let mut buf = [0u8; BUF_SIZE];
            loop {
                println!("[{}] Reading...", name);
                let read = pinned_reader.read(&mut buf).await;
                match read {
                    Ok(bytes_read) => {
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                        if bytes_read == 0 {
                            if now - last_read >= TIMEOUT_THRESHOLD as u64 {
                                println!("[{}] Connection timeout, closing connection.", name);
                                break;
                            }
                            continue;
                        }

                        println!("[{}] Read: {}", name, bytes_read);
                        last_read = now;
                        let received = &buf[..bytes_read];
                        if let Err(e) = pinned_writer.write(&received).await {
                            println!("[{}] Failed to write bytes, closing connection: {:?}", name, e);
                            break;
                        }
                        println!("[{}] Send: {}", name, bytes_read);
                    },
                    Err(e) => {
                        println!("[{}] Failed to read: {:?}", name, e)
                    }
                }
            }
            println!("[{}] Connection closed", name)
        });
    }
}
