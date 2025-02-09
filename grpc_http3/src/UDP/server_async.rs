use tonic::{transport::Server, Request, Response, Status};

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::io::DuplexStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[derive(Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> std::result::Result<Response<HelloReply>, Status> {
        println!("Got a request from {:?}", request.remote_addr());

        let reply = hello_world::HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        println!("Replying with {:?}", reply);
        Ok(Response::new(reply))
    }
}

struct Io {
    socket: UdpSocket,

    // Towards the clients.
    clients_tx: HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>,

    // To receive messages from the clients.
    rx: Receiver<(Vec<u8>, SocketAddr)>,

    // To let clients send messages to the IO thread.
    tx: Sender<(Vec<u8>, SocketAddr)>,

    // DuplexStream with gRPC.
    to_grpc: UnboundedSender<std::result::Result<DuplexStream, String>>,
}

impl Io {
    async fn run(&mut self) -> Result<()> {
        // Main task listens to incoming packets and forwards them on tonic.
        let mut buffer = [0u8; 2000];

        loop {
            // Use `tokio::select` to wake up upon either gRPC data or socket packet.
            tokio::select! {
                Ok(_) = self.socket.readable() => self.handle_socket_readable(&mut buffer).await?,
                Some((msg, to)) = self.rx.recv() => self.handle_from_client_grpc(msg, to).await?,
            }
        }
    }

    async fn handle_socket_readable(&mut self, buf: &mut [u8]) -> Result<()> {
        let (len, from) = match self.socket.try_recv_from(buf) {
            Ok(v) => v,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(());
                }
                return Err(e.into());
            }
        };

        let client_tx = match self.clients_tx.get(&from) {
            Some(v) => v,
            None => {
                // Communication with gRPC.
                let (to_client, from_client) = tokio::io::duplex(1000);

                // Communication with IO.
                let (tx, rx) = mpsc::channel(1000);

                let mut new_client = Client {
                    stream: to_client,
                    to_io: self.tx.clone(),
                    from_io: rx,
                    addr: from,
                };

                // Let the client run on its own.
                tokio::spawn(async move {
                    new_client.run().await.unwrap();
                });

                // Notify gRPC of the new client.
                self.to_grpc.send(Ok(from_client))?;

                self.clients_tx.insert(from, tx);
                self.clients_tx.get_mut(&from).unwrap()
            }
        };

        client_tx.send(buf[..len].to_vec()).await?;

        Ok(())
    }

    async fn handle_from_client_grpc(&mut self, msg: Vec<u8>, to: SocketAddr) -> Result<()> {
        self.socket.send_to(&msg, to).await?;
        Ok(())
    }
}

struct Client {
    stream: DuplexStream,
    to_io: Sender<(Vec<u8>, SocketAddr)>,
    from_io: Receiver<Vec<u8>>,
    addr: SocketAddr,
}

impl Client {
    async fn run(&mut self) -> Result<()> {
        let mut buf = [0u8; 1500];
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => self.handle_grpc_msg(&buf[..len]).await?,
            }
        }
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        self.stream.write(&msg).await?;
        
        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        self.to_io.send((msg.to_vec(), self.addr)).await?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (to_tonic, from_tonic) =
        mpsc::unbounded_channel::<std::result::Result<DuplexStream, String>>();
    let greeter = MyGreeter::default(); //Define the gRPC service.

    // To let clients communicate with the main thread.
    let (tx, rx) = mpsc::channel(1000);

    let addr: SocketAddr = "127.0.0.1:4433".parse().unwrap();
    println!("GreeterServer listening on {:?}", addr);

    let socket = UdpSocket::bind(addr).await?;

    let mut io = Io {
        socket,
        clients_tx: HashMap::new(),
        tx,
        rx,
        to_grpc: to_tonic,
    };

    // Create tonic server builder.
    task::spawn(async move {
        Server::builder()
            .add_service(GreeterServer::new(greeter))
            .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(
                from_tonic,
            ))
            .await
            .unwrap();
    });

    io.run().await?;

    Ok(())
}
