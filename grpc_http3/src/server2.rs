use tonic::{transport::Server, Request, Response, Status};

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

mod quicStream;
use crate::quicStream::QuicStream;
use tokio::sync::mpsc;
use tokio::io::DuplexStream;
use std::collections::HashMap;
use tokio::task;
use tokio::io::{AsyncReadExt, AsyncWriteExt};


//mod quicConnector;

struct Client {
    stream: DuplexStream,
}

type ClientMap = HashMap<std::net::SocketAddr, Client>;

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
    ) -> Result<Response<HelloReply>, Status> {
        println!("Got a request from {:?}", request.remote_addr());

        let reply = hello_world::HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        println!("Replying with {:?}", reply);
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr : String = "127.0.0.1:4433".parse().unwrap();
    let greeter = MyGreeter::default(); //Define the gRPC service

    println!("GreeterServer listening on {}", addr);

    let (tx, recv) = mpsc::unbounded_channel::<Result<DuplexStream, String>>();
    
    //or spawn_blocking ?
    task::spawn(async move { 
        println!("Starting gRPC server");
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);
        let timeout = std::time::Duration::from_millis(10);
        let mut clients = ClientMap::new();

        let mut socket = mio::net::UdpSocket::bind("127.0.0.1:4433".parse().unwrap()).unwrap();
        let mut buf = [0; 65535];
        
        poll.registry()
            .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
            .unwrap();
        
        loop { //global loop
            poll.poll(&mut events, Some(timeout)).unwrap();

            'read: loop { //Read loop that reads packets from the UDP socket and send them on the DuplexStream
                println!("Read loop");
                if events.is_empty() {
                println!("timed out");
                    break 'read;
                }
                
                let (len, from) = match socket.recv_from(&mut buf) {
                    Ok(v) => v,

                    Err(e) => {
                        // There are no more UDP packets to read, so end the read
                        // loop.
                        println!("Nothing to read: {:?}", e);
                        break 'read;
                    },
                };

                match clients.get_mut(&from) {
                    Some(client) => { //Already known client
                        let out = &buf[..len];
                        println!("Received {} bytes from {:?}", out.len(), from);
                        client.stream.write(out).await.unwrap(); //Send data to the gRPC server
                    },
                    None => { //New client
                        let (mut send, mut rcv) = tokio::io::duplex(128);
                        let out = &buf[..len];
                        send.write(out).await.unwrap();
                        println!("Received {} bytes from {:?}", out.len(), from);
                        tx.send(Ok(rcv)).unwrap();
                        let client = Client {
                            stream: send,
                        };
                        
                        clients.insert(from, client);   
                              
                        
                    }
                }
            }
    
            for (ip, client) in clients.iter_mut() { //write loop that reads from the DuplexStream and sends the data to the client
                println!("Write loop");
                'write: loop{
                    let mut buffer = [0; 65535];
                    match client.stream.read(&mut buffer).await {
                        Ok(0) => {
                            println!("Nothing to send to client ");
                            /* 
                            println!("Client {} disconnected", ip);
                            clients.remove(ip);
                            */
                        },
                        Ok(n) => {
                            println!("Put {} bytes in the sync buffer", n);
                            socket.send_to(&buffer[..n], *ip).unwrap();
                        },
                        Err(e) => {
                            println!("Error reading : {:?}", e);
                        }
                    }
                }
            }}
    
            
    
    });

    Server::builder()
        .add_service(GreeterServer::new(greeter))
        .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(recv))
        .await?;


    Ok(())
}