use std::net::{SocketAddr, SocketAddrV4};

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use echo::{echo_client::EchoClient, EchoRequest};
use filetransfer::file_service_client::FileServiceClient;
use filetransfer::{FileData, FileRequest};

use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Endpoint,Uri};
use tower::service_fn;
use hyper_util::rt::tokio::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc;
use tokio::task;
use log::{info, error, debug};
use tokio::time::sleep;
use std::time::{Duration, Instant};
use docopt::Docopt;
use tonic::transport::Channel;
use tokio::fs::{self,File};
use std::collections::HashMap;
use tokio::task::JoinHandle;
use slab::Slab;
use log::{trace,warn};
use std::net::{IpAddr, Ipv4Addr};
use quiche::{PathId, CIDSeq};
use std::sync::Arc;

use quiche::h3::NameValue;
use tokio::net::TcpSocket;
use ring::rand::*;
use rand::seq::SliceRandom;
use rand::thread_rng;

const MAX_DATAGRAM_SIZE: usize = 1500;
const MAX_GRPC_BUFFER_DATA_SIZE: usize = 100_001;
const MAX_BRIDGING_BUFFER_SIZE: usize = 100_000;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

pub mod echo {
    tonic::include_proto!("echo");
}

pub mod filetransfer {
    tonic::include_proto!("filetransfer");
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;


#[tokio::main]
async fn main() -> Result<()> {
    let proto = "fileexchange";
    let server_addr = "127.0.0.1:6368";
    let mut file_path = "../file_examples/file".to_string();
    let n = 80;
    let dur = 0;

    let channel = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
        .connect_with_connector_lazy(service_fn(|uri: Uri| async {
            let (client, server) = tokio::io::duplex(1_000_000);
            task::spawn(async move {
                let (to_client, from_io) = mpsc::channel(1000);
                let (to_io, from_client) = mpsc::channel(1000);

                let mut new_client = Client {
                    stream: client,
                    to_io: to_io,
                    from_io: from_io,
                };
            
                // Let the client run on its own.
                tokio::spawn(async move {
                    new_client.run().await.unwrap();
                });

                run_client(uri, to_client, from_client).await.unwrap();
            });

            Ok::<_, std::io::Error>(TokioIo::new(server))
    }));

    let start = Instant::now();

    if proto == "echo" {
        let mut client = EchoClient::new(channel);
        bidirectional_streaming_echo(&mut client, 100).await;
        
    } else if proto == "helloworld" {
        let mut client = GreeterClient::new(channel);

        let request = tonic::Request::new(HelloRequest {
            name: "Maxime".into(),
        });
        let response = client.say_hello(request).await?;

        println!("RESPONSE={:?}", response.into_inner().message);
               
    } else if proto == "filetransfer" {
        let mut client = FileServiceClient::new(channel);

        // Upload a file
        if cfg!(target_os = "windows") {
            file_path = "./file_examples/big.txt".to_string();
        }
        
        let mut file = File::open(file_path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        let request = tonic::Request::new(FileData {
            filename: "uploaded_example".into(),
            data: buffer,
        });

        let response = client.upload_file(request).await?.into_inner();
        println!("Upload Response: {}", response.message);
        
        // Download a file
        let request = tonic::Request::new(FileRequest {
            filename: "uploaded_example".into(),
        });

        let response = client.download_file(request).await?.into_inner();
        let mut new_file = File::create("downloaded_example").await?;
        new_file.write_all(&response.data).await?;

        println!("File downloaded successfully!"); 
        
    } else if proto == "fileexchange" {
        let mut client = FileServiceClient::new(channel);

        for i in 0..n {

            sleep(Duration::from_millis(dur)).await;

            let s = Instant::now();
            // exchange a file
            if cfg!(target_os = "windows") {
                file_path = "./file_examples/big.txt".to_string();
            }
            let mut file = File::open(file_path.clone()).await?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).await?;

            let request = tonic::Request::new(FileData {
                filename: "uploaded_example".into(),
                data: buffer,
            });

            let response = client.exchange_file(request).await?.into_inner();

            let mut new_file = File::create("downloaded_example").await?;
            new_file.write_all(&response.data).await?;

            println!("File exchanged successfully!");

            let d = s.elapsed();
            let c = start.elapsed();
            println!("Time elapsed: ({:?} ,{:?})", c.as_secs_f64(), d.as_secs_f64());
        }

    } else {
        panic!("Invalid protoBuf");
    }
        
    let duration = start.elapsed();
    println!("Total time elapsed: {:?}", duration.as_secs_f64());

    Ok(())
}

pub async fn run_client(uri: Uri, to_client: Sender<Vec<u8>>, mut from_client: Receiver<Vec<u8>>,) -> Result<()> {
    
    let addr = "127.0.0.1:3434".parse().unwrap();

    let socket = TcpSocket::new_v4()?;
    socket.bind(addr)?;

    socket.set_keepalive(true)?;
    socket.set_reuseaddr(true)?;
    socket.set_reuseport(true)?;
    socket.set_nodelay(true)?;

    let mut stream = socket.connect("127.0.0.1:4545".parse().expect("Problem parsing the server adresse.")).await?;

    let mut buffer = [0u8; 1500];
    loop {
        tokio::select! {
            Ok(_) = stream.readable() => {
                let len = match stream.try_read(&mut buffer[..]) {
                    Ok(v) => v,
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            continue;
                        }
                        return Err(e.into());
                    }
                };
                to_client.send((&buffer[..len]).to_vec()).await?;
            },

            Some(data) = from_client.recv() => {
                stream.write(&data).await?;
            },
        }
    }
}






pub struct Client {
    pub stream: DuplexStream,
    pub to_io: Sender<Vec<u8>>,
    pub from_io: Receiver<Vec<u8>>,
}

impl Client {
    // Handle the communication between Quic (synchronous) thread and gRPC (asynchronous) thread.
    pub async fn run(&mut self) -> Result<()> {
        let mut buf = [0u8; MAX_BRIDGING_BUFFER_SIZE];
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => {
                    if len == 0 {
                        println!("[CLIENT] Received 0 bytes, closing connection");
                        return Ok(());
                    }

                    self.handle_grpc_msg(&buf[..len]).await?
                },
            }

            if self.from_io.is_closed() {
                return Ok(());
            }
        }
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        self.stream.write(&msg).await?;

        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        self.to_io.send(msg.to_vec()).await?;
    
        Ok(())
    }
}

fn hex_dump(buf: &[u8]) -> String {
    let vec: Vec<String> = buf.iter().map(|b| format!("{b:02x}")).collect();

    vec.join("")
}

pub fn hdrs_to_strings(hdrs: &[quiche::h3::Header]) -> Vec<(String, String)> {
    hdrs.iter()
        .map(|h| {
            let name = String::from_utf8_lossy(h.name()).to_string();
            let value = String::from_utf8_lossy(h.value()).to_string();

            (name, value)
        })
        .collect()
}

fn echo_requests_iter() -> impl Stream<Item = EchoRequest> {
    tokio_stream::iter(1..usize::MAX).map(|i| EchoRequest {
        message: format!("msg {:02}", i),
    })
}

async fn streaming_echo(client: &mut EchoClient<Channel>, num: usize) {
    let stream = client
        .server_streaming_echo(EchoRequest {
            message: "foo".into(),
        })
        .await
        .unwrap()
        .into_inner();

    // stream is infinite - take just 5 elements and then disconnect
    let mut stream = stream.take(num);
    while let Some(item) = stream.next().await {
        println!("\treceived: {}", item.unwrap().message);
    }
    // stream is dropped here and the disconnect info is sent to server
}

async fn bidirectional_streaming_echo(client: &mut EchoClient<Channel>, num: usize) {
    let in_stream = echo_requests_iter().take(num);

    let response = client
        .bidirectional_streaming_echo(in_stream)
        .await
        .unwrap();

    let mut resp_stream = response.into_inner();

    while let Some(received) = resp_stream.next().await {
        let received = received.unwrap();
        println!("\treceived message: `{}`", received.message);
    }
}

async fn bidirectional_streaming_echo_throttle(client: &mut EchoClient<Channel>, dur: Duration) {
    let in_stream = echo_requests_iter().throttle(dur);

    let response = client
        .bidirectional_streaming_echo(in_stream)
        .await
        .unwrap();

    let mut resp_stream = response.into_inner();

    while let Some(received) = resp_stream.next().await {
        let received = received.unwrap();
        println!("\treceived message: `{}`", received.message);
    }
}

