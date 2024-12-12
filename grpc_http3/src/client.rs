use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::{Endpoint,Uri};
mod quicConnector;
use crate::quicConnector::QuicConnector;
use tokio::io::DuplexStream;
use tower::service_fn;
use hyper_util::rt::tokio::TokioIo;
use tokio::task;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /*
    let connector = QuicConnector::new().expect("Failed to create connector");


    let channel = Endpoint::from_static("http://[::1]:50051")
        .connect_with_connector(connector).await?;
    */
    
    let channel = Endpoint::from_static("https://127.0.0.1:4433")
        .connect_with_connector(service_fn(|_: Uri| async {
            let (mut client, server) = tokio::io::duplex(128);

            task::spawn(async move { 
                let mut buf = [0; 65535];

                let peer_addr: std::net::SocketAddr = "127.0.0.1:4433".parse().expect("Failed to parse address");
                

                let mut poll = mio::Poll::new().unwrap();
                let mut events = mio::Events::with_capacity(1024);
                let timeout = std::time::Duration::from_millis(1);

                let mut socket = mio::net::UdpSocket::bind("0.0.0.0:0".parse().unwrap()).unwrap();
                poll.registry()
                .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
                .unwrap();

                loop{
                    poll.poll(&mut events, Some(timeout)).unwrap();
                    'read: loop {
                        println!("Read loop");
                        if events.is_empty() {
                            println!("No events");
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

                    let out = &buf[..len];
                    println!("Received {} bytes from {:?}", out.len(), from);
                    client.write_all(out).await.unwrap(); //Send data to the gRPC server

                    }

                    'write: loop{
                        println!("Write loop");
                        let mut out = [0; 65535];
                        match client.read(&mut out).await {
                            Ok(0) => {
                                println!("Nothing to send to {}", peer_addr);
                                /* 
                                println!("Client {} disconnected", ip);
                                clients.remove(ip);
                                */
                            },
                            Ok(n) => {
                                println!("Send {} bytes to {:?}", n, peer_addr);
                                socket.send_to(&out[..n], peer_addr).unwrap();
                            },
                            Err(e) => {
                                println!("Error reading from client {}: {:?}", peer_addr, e);
                            }
                        }
                    }
                }
            });


            Ok::<_, std::io::Error>(TokioIo::new(server))
        }))
        .await?;
    /* 
    let (mut client, mut server) = tokio::io::duplex(64);

    let channel = Endpoint::try_from("http://[::]:50051")?
    .connect_with_connector(service_fn(|_: Uri| async {
        let path = "/tmp/tonic/helloworld";

        // Connect to a Uds socket
        Ok::<_, std::io::Error>(TokioIo::new(DuplexStream::new(client, server).await?))
    }))
    .await?;
    */
    let mut client = GreeterClient::new(channel);
     

    let request = tonic::Request::new(HelloRequest {
        name: "Maxime".into(),
    });
   

    let response = client.say_hello(request).await?;

    println!("RESPONSE={:?}", response);

    Ok(())
}