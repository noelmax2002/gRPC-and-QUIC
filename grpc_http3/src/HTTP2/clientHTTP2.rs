use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use echo::{echo_client::EchoClient, EchoRequest};
use filetransfer::file_service_client::FileServiceClient;
use filetransfer::{FileData, FileRequest};

use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_stream::{Stream, StreamExt};
use docopt::Docopt;
use core::net::IpAddr;
use tokio::time::sleep;


#[cfg(feature = "tls")]
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

pub mod echo {
    tonic::include_proto!("echo");
}

pub mod filetransfer {
    tonic::include_proto!("filetransfer");
}

// Write the Docopt usage string.
const USAGE: &'static str = "
Usage: client [options]

Options:
    -s --sip ADDRESS ...            Server IPv4 address and port [default: 127.0.0.1:4433].
    -A --address ADDR ...           Client potential multiple addresses.
    --ca-cert PATH                  Path to ca.pem [default: ./HTTP2/tls/ca.pem].
    --file FILE                     File to upload [default: ../swift_file_examples/small.txt].
    -p --proto PROTOCOL             Choose the protoBuf to use [default: helloworld].
    -n --num NUM                    Number of requests to send [default: 10].
    -t --time DURATION              Duration between each request [default: 500].
";

//[::1]:50051
//192.168.1.7:8080

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {

    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let proto = args.get_str("--proto").to_string();
    
    let mut ca_cert: String = args.get_str("--ca-cert").parse().unwrap();
    let mut file_path: String = args.get_str("--file").parse().unwrap();
    let mut n = args.get_str("--num").parse().unwrap();
    let dur = args.get_str("--time").parse::<u64>().unwrap();

    let mut server_addrs: Vec<&str> = args
        .get_vec("--sip");

   
    let mut server_addr: String = server_addrs[0].to_string();
  

    if cfg!(target_os = "windows") {
        ca_cert = "./src/HTTP2/tls/ca.pem".to_string();
    }
    let pem = std::fs::read_to_string(ca_cert)?;
    let ca = Certificate::from_pem(pem);


    let tls = ClientTlsConfig::new()
        .ca_certificate(ca)
        .domain_name("example.com");

    let mut addrs: Vec<IpAddr> = args
        .get_vec("--address")
        .into_iter()
        .filter_map(|a| a.parse().ok())
        .collect();

    let mut channel;

    if addrs.len() == 2 && server_addrs.len() == 1 {
        let endpoint1 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[0]))
            .timeout(Duration::from_secs(5))
            .tls_config(tls.clone())?;
        
        let endpoint2 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[1]))
            .timeout(Duration::from_secs(5))
            .tls_config(tls.clone())?;

        let endpoints = vec![endpoint1, endpoint2];

        channel = Channel::balance_list(endpoints.into_iter());
    } else if addrs.len() == 2 && server_addrs.len() == 2 {
        let endpoint1 = Endpoint::from_shared(format!("https://{}", server_addrs[0]).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[0]))
            .timeout(Duration::from_secs(5))
            .tls_config(tls.clone())?;
        
        let endpoint2 = Endpoint::from_shared(format!("https://{}", server_addrs[1]).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[1]))
            .timeout(Duration::from_secs(5))
            .tls_config(tls.clone())?;

        let endpoints = vec![endpoint1, endpoint2];

        channel = Channel::balance_list(endpoints.into_iter());
    } else {
        channel = Channel::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
        .tls_config(tls)?
        .connect()
        .await?;

    }

    

    let start = Instant::now();
        
    if proto.as_str() == "echo" {
        let mut client = EchoClient::new(channel);
        bidirectional_streaming_echo(&mut client, 50).await;
    } else if proto.as_str() == "helloworld" {
        let mut client = GreeterClient::new(channel);

        let request = tonic::Request::new(HelloRequest {
            name: "Maxime".into(),
        });
        
    
        let response = client.say_hello(request).await.unwrap();
        println!("response: {:?}", response.get_ref().message);

    } else if proto.as_str() == "filetransfer" {
        let mut client = FileServiceClient::new(channel);

        // Upload a file
        if cfg!(target_os = "windows") {
            file_path = "./swift_file_examples/big.txt".to_string();
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

    } else if proto.as_str() == "fileexchange" {
        let mut client = FileServiceClient::new(channel);

        for i in 0..n {

            sleep(Duration::from_millis(dur)).await;

            let s = Instant::now();
            // exchange a file
            if cfg!(target_os = "windows") {
                file_path = "./swift_file_examples/big.txt".to_string();
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
            println!("Time elapsed: ({:?} ,{:?})", c.as_secs_f64(),d.as_secs_f64());
        }

    } else {
        panic!("Invalid protoBuf");
    }

        
    let duration = start.elapsed();
    println!("Total time elapsed: {:?}", duration.as_secs_f64());

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

