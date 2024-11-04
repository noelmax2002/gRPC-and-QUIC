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
    //let mut quiche_config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
    //quiche_config.verify_peer(false);  // Adjust according to security needs
    
    
    let connector = QuicConnector::new().expect("Failed to create connector");


    let channel = Endpoint::from_static("http://[::1]:50051")
        .connect_with_connector_lazy(connector);
    
    /* 
    let channel = Endpoint::try_from("http://[::]:50051")?
    .connect_with_connector(service_fn(|_: Uri| async {

        Ok::<_, std::io::Error>(empty())
    }))
    .await?;*/

    let mut client = GreeterClient::new(channel);
     

    let request = tonic::Request::new(HelloRequest {
        name: "Maxime".into(),
    });
   

    let response = client.say_hello(request).await?;

    println!("RESPONSE={:?}", response);

    Ok(())
}