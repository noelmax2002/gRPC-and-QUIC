use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::Endpoint;
mod quicConnector;
use crate::quicConnector::QuicConnector;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
        
    let connector = QuicConnector::new().expect("Failed to create connector");


    let channel = Endpoint::from_static("http://[::1]:50051")
        .connect_with_connector(connector).await?;

    let mut client = GreeterClient::new(channel);
     

    let request = tonic::Request::new(HelloRequest {
        name: "Maxime".into(),
    });
   

    let response = client.say_hello(request).await?;

    println!("RESPONSE={:?}", response);

    Ok(())
}