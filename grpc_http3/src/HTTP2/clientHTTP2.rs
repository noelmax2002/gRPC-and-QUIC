use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "tls")]
use tonic::transport::{Certificate, Channel, ClientTlsConfig};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}
//[::1]:50051
//192.168.1.7:8080

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

    let pem = std::fs::read_to_string("./src/HTTP2/tls/ca.pem")?;
    let ca = Certificate::from_pem(pem);

    let tls = ClientTlsConfig::new()
        .ca_certificate(ca)
        .domain_name("example.com");

    let mut channel = Channel::from_static("https://127.0.0.1:4433")
        .tls_config(tls)?
        .connect()
        .await?;

    let mut client = GreeterClient::new(channel);

    let request = tonic::Request::new(HelloRequest {
        name: "Maxime".into(),
    });

    let response = client.say_hello(request).await?;

    println!("RESPONSE={:?}", response);
    let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        println!("Time elapsed: {:?}", end - start);

    Ok(())
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[tokio::test]
    // Server must be running before running this test.
    async fn hello_world_test() -> Result<(), Box<dyn std::error::Error>> {
        

        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let mut client = GreeterClient::connect("http://[::1]:50051").await?;

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
