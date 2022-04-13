pub mod proxy_server {
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::fmt::Debug;
    use std::net::SocketAddr;
    use std::sync::{Arc};
    use std::time::{SystemTime, UNIX_EPOCH};
    use hyper::{Client, Server, Body, Method, Request, Response};
    use hyper::server::conn::AddrStream;
    use hyper::service::{make_service_fn, service_fn};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;
    use tokio::time::{Duration, sleep, timeout};
    use uuid::Uuid;

    const BUF_SIZE: usize = 1024;
    const TIMEOUT_THRESHOLD_SECS: u64 = 2;


    #[derive(Debug, Clone)]
    pub struct Connection {
        pub id: String,
        client: String,
        host: String
    }

    impl Connection {
        pub fn create(client: String, host: String) -> Connection {
            let id = Uuid::new_v4().to_string();
            Connection {
                id, client, host
            }
        }
    }

    pub struct ProxyServer {
        open_connections: Arc<Mutex<HashMap<String, Connection>>>
    }

    impl ProxyServer {
        pub fn create() -> ProxyServer {
            ProxyServer {
                open_connections: Arc::from(Mutex::from(HashMap::new()))
            }
        }

        pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let make_svc = make_service_fn(|socket: &AddrStream| {
                let open_connections_mut = Arc::clone(&self.open_connections);
                let client_addr = Arc::new(socket.remote_addr().to_string());
                async move {
                    Ok::<_, Infallible>(
                        service_fn(move |req: Request<Body>| {
                            let open_connections_mut = open_connections_mut.clone();
                            let client_addr = client_addr.clone();
                            async move {
                                return handle_request(client_addr, req, open_connections_mut).await;
                            }
                        })
                    )
                }
            });

            let addr = ([127, 0, 0, 1], 3000).into();
            let server = Server::bind(&addr).serve(make_svc);
            println!("Listening on http://{}", addr);

            let open_connections_ref = Arc::clone(&self.open_connections);
            tokio::task::spawn(async move {
                loop {
                    let open_connections = open_connections_ref.lock().await;
                    println!("### open connections: {:?}", open_connections);
                    sleep(Duration::from_millis(1000)).await;
                }
            });

            println!("Monitor task started");

            server.await?;

            Ok(())
        }
    }

    async fn handle_request<'a>(
        client_address: Arc<String>,
        mut req: Request<Body>,
        mut open_connections_mut: Arc<Mutex<HashMap<String, Connection>>>
    ) -> Result<Response<Body>, hyper::Error> {
        let client = Client::new();
        println!("### Request: {:?}", req);

        let res = match req.method() {
            &Method::GET => Ok(client.get(req.uri().clone()).await?),
            &Method::CONNECT => {
                let res = Response::new(Body::empty());

                let mut open_connections_clone = open_connections_mut.clone();
                let mut open_connections = open_connections_clone.lock().await;
                let connection = Connection::create(client_address.to_string(), req.uri().clone().to_string());
                let connection_id = connection.id.clone();
                open_connections.insert(connection_id.clone(), connection);
                // drop(open_connections);

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

                            tokio::join!(client_handle, host_handle);

                            let mut open_connections_clone = open_connections_mut.clone();
                            let mut open_connections = open_connections_clone.lock().await;
                            open_connections.remove(&connection_id.clone());
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
                // println!("[{}] Reading...", name);
                let read = timeout(Duration::from_secs(TIMEOUT_THRESHOLD_SECS), pinned_reader.read(&mut buf)).await;
                if let Err(e) = read {
                    println!("Read timeout: {:?}", e);
                    break;
                }
                let read_res = read.unwrap();
                match read_res {
                    Ok(bytes_read) => {
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                        if bytes_read == 0 {
                            if now - last_read >= TIMEOUT_THRESHOLD_SECS as u64 {
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
