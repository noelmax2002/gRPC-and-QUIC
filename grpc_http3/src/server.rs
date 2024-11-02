use tonic::{transport::Server, Request, Response, Status};

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

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
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse().unwrap();
    let greeter = MyGreeter::default();

    println!("GreeterServer listening on {}", addr);

    Server::builder()
        .add_service(GreeterServer::new(greeter))
        .serve_with_incoming(addr)
        .await?;

    Ok(())
}

async fn http3_listener() -> Result<TokioUdpSocket, Box<dyn std::error::Error>> {
     
    
    let local: SocketAddr = "0.0.0.1:50051".parse()?; 
    
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
    
    let scid = quiche::ConnectionId::from_ref(&[0xba, 0xad, 0xf0, 0x0d]);
    let mut conn = quiche::accept(&scid, None, socket, socket, &mut config)?;
    
    Ok(socket)
    
}
