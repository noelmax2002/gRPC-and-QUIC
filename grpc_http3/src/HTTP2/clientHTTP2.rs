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
use tokio::sync::mpsc::Sender;
use std::net::SocketAddr;
use netsock::get_sockets;
use netsock::family::AddressFamilyFlags;
use netsock::protocol::ProtocolFlags;
use netsock::socket::ProtocolSocketInfo;
use netsock::socket::TcpSocketInfo;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

#[cfg(feature = "tls")]
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::transport::channel::Change;
use tokio::time::timeout;
use netsock;


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
    --ptimer DURATION               Duration of the timer for link probing [default: 500].
    --rtime DURATION                Maximum duration of a request [default: 1000].
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
    let ptimer = args.get_str("--ptimer").parse::<u64>().unwrap();
    let rtime = args.get_str("--rtime").parse::<u64>().unwrap();

    let mut server_addrs: Vec<&str> = args.get_vec("--sip");

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

    let problem = Arc::new(AtomicBool::new(false));
    let problem_clone = problem.clone();

    println!("Client address: {:?}", addrs);
    println!("Server addresses: {:?}", server_addrs);

    if addrs.len() == 2 && server_addrs.len() == 1 {
        println!("Using multiple client addresses");
        let endpoint1 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[0]))
            .tls_config(tls.clone())?;
        
        let endpoint2 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[1]))
            .tls_config(tls.clone())?;

        let endpoints = vec![endpoint1, endpoint2];

        channel = Channel::balance_list(endpoints.into_iter());
    } else if addrs.len() == 2 && server_addrs.len() == 2 {
        println!("Using multiple server addresses");
        let endpoint1 = Endpoint::from_shared(format!("https://{}", server_addrs[0]).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[0]))
            .tls_config(tls.clone())?;
        
        let endpoint2 = Endpoint::from_shared(format!("https://{}", server_addrs[1]).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[1]))
            .tls_config(tls.clone())?;

        if ptimer == 0 {
            let endpoints = vec![endpoint1, endpoint2];

            channel = Channel::balance_list(endpoints.into_iter());
        } else {
            let (channel_new, rx) = Channel::balance_channel(10);
            channel = channel_new;
            
            let rx1 = rx.clone();
            let rx2 = rx.clone();/*
            let socket: SocketAddr = server_addrs[0].parse::<SocketAddr>().unwrap().clone();
            let addr = socket.ip();
            println!("Server address: {:?}", addr);
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                println!("Added endpoint 1");
                let change = Change::Insert("1", endpoint1);
                let res = rx1.send(change).await;
                println!("{:?}", res);
                //continue to monitor endpoint

                /*
                tokio::time::sleep(Duration::from_millis(ptimer)).await;
                let change = Change::Remove("1");
                let res = rx1.send(change).await;
                println!("{:?}", res);
                */

                
                println!("Pinging server address: {:?}", addr);
                loop {
                    tokio::time::sleep(Duration::from_millis(ptimer)).await;
                    let af_flags = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
                    let proto_flags = ProtocolFlags::TCP | ProtocolFlags::UDP;

                    match get_sockets(af_flags, proto_flags) {
                        Ok(sockets) => {
                            //filter socket that matches the address
                            let filtered_sockets: Vec<_> = sockets.clone()
                                .into_iter()
                                .filter(|s| s.local_addr() == addr && s.local_port() == socket.port())
                                .collect();

                            for socket in filtered_sockets { 
                                //let socket_info = ProtocolSocketInfo::Tcp(socket.protocol_socket_info);
                                //let socket_info: TcpSocketInfo = socket.protocol_socket_info;
                                let socket_info = match socket.protocol_socket_info {
                                    ProtocolSocketInfo::Tcp(socket_info) => socket_info,
                                    _ => continue,
                                };
                                println!("Socket: {:?}", socket_info.state);
                            }
                        },
                        Err(e) => eprintln!("Failed to get sockets: {}", e),
                    }
                    
                    /*
                    tokio::time::sleep(Duration::from_millis(ptimer)).await;
                    let data = [1,2,3,4];  // ping data
                    let timeout = Duration::from_secs(1);
                    let options = ping_rs::PingOptions { ttl: 128, dont_fragment: true };
                    let result = ping_rs::send_ping(&addr, timeout, &data, Some(&options));
                    match result {
                        Ok(reply) => println!("Reply from {}: bytes={} time={}ms TTL={}", reply.address, data.len(), reply.rtt, options.ttl),
                        Err(e) => {
                            println!("{:?}", e);
                            //path switching
                            /*
                            println!("Added endpoint 2");
                            let change = Change::Insert("2", endpoint2.clone());
                            let res = rx2.send(change).await;
                            println!("{:?}", res);
                            
                            println!("Removed first endpoint");
                            let change = Change::Remove("1");
                            let res = rx1.send(change).await;
                            println!("{:?}", res);
                            */
                        }
                    }*/
                }
            
            });*/
    
            
            tokio::spawn(async move {
                //tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                println!("Added endpoint 1");
                let change = Change::Insert("1", endpoint1.clone());
                let res = rx2.send(change).await;
                println!("{:?}", res);

                //monitor endpoint
                let mut backup_link = false;
                loop {
                    tokio::time::sleep(Duration::from_millis(ptimer)).await;
                    //println!("Checking health of connection");
                    if backup_link{
                        
                        println!("Backup link is active");
                        println!("Try reconnect to endpoint 1");
                        let new_endpoint1 = endpoint1.clone();
                        match new_endpoint1.connect().await {
                            Ok(_) => println!("Connected to endpoint 1"),
                            Err(e) => {println!("Failed to connect to endpoint 1: {:?}", e); continue;},
                        }
                        println!("Added endpoint 1");
                        let change = Change::Insert("1", new_endpoint1);
                        let res = rx2.send(change).await;
                        println!("{:?}", res);
                        println!("Removing endpoint 2");
                        let change = Change::Remove("2");
                        let res = rx2.send(change).await;
                        println!("{:?}", res);
                        problem_clone.swap(false, SeqCst);
                        backup_link = false;
                        

                    } else if problem_clone.load(SeqCst) {
                        println!("Problem detected, adding endpoint 2");
                        let change = Change::Insert("2", endpoint2.clone());
                        let res = rx2.send(change).await;
                        println!("{:?}", res);
                        println!("Problem detected, removing endpoint 1");
                        let change = Change::Remove("1");
                        let res = rx2.send(change).await;
                        println!("{:?}", res);
                        backup_link = true;
                        problem_clone.swap(false, SeqCst);
                    }
                }

                /*
                tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;
                println!("Added endpoint 1");
                match endpoint1.connect().await {
                    Ok(_) => println!("Connected to endpoint 1"),
                    Err(e) => println!("Failed to connect to endpoint 1: {:?}", e),
                }
                let change = Change::Insert("1", endpoint1);
                let res = rx2.send(change).await;
                println!("{:?}", res);
                //continue to monitor endpoint
                */

                
            });
        }

    } else {
        println!("Using single server address");
        channel = Channel::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
        .tls_config(tls)?
        .connect_lazy();

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
        /*
        // Download a file
        let request = tonic::Request::new(FileRequest {
            filename: "uploaded_example".into(),
        });

        let response = client.download_file(request).await?.into_inner();
        let mut new_file = File::create("downloaded_example").await?;
        new_file.write_all(&response.data).await?;

        println!("File downloaded successfully!");*/

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

            let mut retry = true;
            let mut response;
            while retry {
                let request = tonic::Request::new(FileData {
                    filename: "uploaded_example".into(),
                    data: buffer.clone(),
                });

                /*
                response =  match client.exchange_file(request).await {
                    Ok(r) => {retry = false; r.into_inner()},
                    Err(e) => {
                        println!("Error: {:?}", e);
                        continue;
                    }
                };*/

                // Cancelling the request by dropping the request future after 1 second
                
                response = match timeout(Duration::from_millis(rtime), client.exchange_file(request)).await {
                    Ok(response) => {retry = false; response?.into_inner()},
                    Err(_) => {
                        println!("Cancelled request after 1s");
                        problem.swap(true, SeqCst);
                        continue;
                    }
                };

                /*
                response = client.exchange_file(request).await?.into_inner();
                retry = false;*/

                let mut new_file = File::create("downloaded_example").await?;
                new_file.write_all(&response.data).await?;
            }

        
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

