use std::net::{SocketAddr, SocketAddrV4};

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tokio::net::UdpSocket;
use tonic::transport::{Endpoint,Uri};
use tower::service_fn;
use hyper_util::rt::tokio::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::task;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> Result<()> {

    let channel = Endpoint::from_static("https://127.0.0.1:4433")
        .connect_with_connector(service_fn(|uri: Uri| async {
            let (client, server) = tokio::io::duplex(12000);
            task::spawn(async move {
                run_client(uri, client).await.unwrap();
            });

            Ok::<_, std::io::Error>(TokioIo::new(server))
    })).await?;

    let mut client = GreeterClient::new(channel);
     

    let request = tonic::Request::new(HelloRequest {
        name: "Maxime".into(),
    });
   

    let response = client.say_hello(request).await?;

    println!("RESPONSE={:?}", response);

    Ok(())
}

async fn run_client(uri: Uri, mut to_grpc: DuplexStream) -> Result<()> {
    let host = SocketAddr::V4(SocketAddrV4::new(uri.host().unwrap().parse().unwrap(), uri.port().unwrap().into()));
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let mut buffer = [0u8; 1500];
    loop {
        tokio::select! {
            Ok(_) = socket.readable() => {
                let (len, _from) = match socket.try_recv_from(&mut buffer[..]) {
                    Ok(v) => v,
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            continue;
                        }
                        return Err(e.into());
                    }
                };

                // Send the message to gRPC.
                to_grpc.write(&buffer[..len]).await?;
            },

            Ok(len) = to_grpc.read(&mut buffer[..]) => {
                socket.send_to(&buffer[..len], host).await?;
            },
        }
    }
}
    
#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[tokio::test]
    // Server must be running before running this test.
    async fn hello_world_test() -> Result<()> {
        
        let channel = Endpoint::from_static("https://127.0.0.1:4433")
        .connect_with_connector(service_fn(|uri: Uri| async {
            let (client, server) = tokio::io::duplex(12000);
            task::spawn(async move {
                run_client(uri, client).await.unwrap();
            });

            Ok::<_, std::io::Error>(TokioIo::new(server))
    })).await?;


        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let mut client = GreeterClient::new(channel);

        let request = tonic::Request::new(HelloRequest {
            name: "Maxime".into(),
        });
        
        let response = client.say_hello(request).await.unwrap();

        let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        println!("Time elapsed: {:?}", end - start);

        assert_eq!(response.get_ref().message, "Hello Maxime!");
    
        Ok(())
    }
}

