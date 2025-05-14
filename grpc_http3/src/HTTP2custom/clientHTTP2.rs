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
use tokio::net::{TcpStream, TcpSocket};
use tokio::io::{AsyncRead, AsyncWrite};
use hyper_util::rt::TokioIo;
use std::{pin::Pin, task::{Context, Poll}, future::Future};
use std::io;
use std::net::TcpStream as StdTcpStream;

#[cfg(target_os = "linux")]
use libc::{getsockopt, socklen_t, tcp_info, SOL_TCP, TCP_INFO};

use std::os::unix::io::{AsRawFd, FromRawFd};

use tonic::transport::{Channel, Endpoint, Uri};
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
    --ptimer DURATION               Duration of the timer for ping request [default: 100].
    --rtime DURATION                Maximum duration of a request [default: 1000].
";

//[::1]:50051
//192.168.1.7:8080

struct TcpConnector {
    stream: Option<TcpStream>,
}

#[derive(Debug)]
pub struct TcpStats {
    pub rtt_us: u32,         // Smoothed RTT in microseconds
    pub rtt_var_us: u32,     // RTT variance in microseconds
    pub retransmits: u32,    // Number of retransmissions
}

impl tower::Service<Uri> for TcpConnector {
    type Response = TokioIo<TcpStream>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _dst: Uri) -> Self::Future {
        let stream = self.stream.take().expect("Only usable once");
        Box::pin(async move {
            Ok(TokioIo::new(stream))
        })
    }
}

#[cfg(target_os = "linux")]
pub fn get_tcp_stats(stream: &TcpStream) -> Option<TcpStats> {
    let fd = stream.as_raw_fd();
    let mut info = std::mem::MaybeUninit::<tcp_info>::uninit();
    let mut len = std::mem::size_of::<tcp_info>() as socklen_t;

    let ret = unsafe { getsockopt(fd, SOL_TCP, TCP_INFO, info.as_mut_ptr() as *mut _, &mut len) };

    if ret == 0 {
        let tcp_info = unsafe { info.assume_init() };
        Some(TcpStats {
            rtt_us: tcp_info.tcpi_rtt,           
            rtt_var_us: tcp_info.tcpi_rttvar,    
            retransmits: tcp_info.tcpi_retrans,
        })
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
pub fn get_tcp_stats(stream: &TcpStream) -> Option<TcpStats> {
    None
}


/// Duplicate a `tokio::net::TcpStream` by duplicating the FD.
fn clone_tokio_tcp_stream(orig: &TcpStream) -> io::Result<TcpStream> {
    // 1) get the raw fd
    let fd = orig.as_raw_fd();

    // 2) dup it
    let new_fd = unsafe { libc::dup(fd) };
    if new_fd < 0 {
        return Err(io::Error::last_os_error());
    }

    // 3) build a std::net::TcpStream from the new fd
    //    SAFETY: new_fd is a valid duplicated fd
    let std_stream = unsafe { StdTcpStream::from_raw_fd(new_fd) };

    // 4) set nonblocking so Tokio can use it
    std_stream.set_nonblocking(true)?;

    // 5) convert into a Tokio TcpStream
    TcpStream::from_std(std_stream)
}

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

    let mut addrs: Vec<IpAddr> = args
        .get_vec("--address")
        .into_iter()
        .filter_map(|a| a.parse().ok())
        .collect();

    let mut channel;
    let mut channel2;

    let problem = Arc::new(AtomicBool::new(false));
    let problem_clone = problem.clone();

    let change_channel = Arc::new(AtomicBool::new(false));
    let change_channel_clone = change_channel.clone();

    let mut monitoring = false;

    println!("Client address: {:?}", addrs);
    println!("Server addresses: {:?}", server_addrs);

    if addrs.len() == 2 && server_addrs.len() == 1 {
        
        println!("Using multiple client addresses");
        let endpoint1 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[0]));
        
        let endpoint2 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
            .local_address(Some(addrs[1]));

        let endpoints = vec![endpoint1, endpoint2];

        channel = Channel::balance_list(endpoints.into_iter());
        channel2 = channel.clone();
    } else if addrs.len() == 2 && server_addrs.len() == 2 {
        monitoring = true;
        let socket = TcpSocket::new_v4()?;
        socket.bind(SocketAddr::new(addrs[0], 3355))?;
        let stream = socket.connect(server_addrs[0].clone().parse().expect("Problem parsing the server adresse.")).await?;
        let stream_for_stats = clone_tokio_tcp_stream(&stream)?;
        let connector = TcpConnector { stream: Some(stream) };
        channel = Endpoint::from_shared(format!("https://{}", server_addrs[0]).to_string()).expect("Invalid URL").connect_with_connector_lazy(connector);

        let socket2 = TcpSocket::new_v4()?;
        socket2.bind(SocketAddr::new(addrs[1], 8080))?;
        let stream2 = socket2.connect(server_addrs[1].clone().parse().expect("Problem parsing the server adresse.")).await?;
        let stream2_for_stats = clone_tokio_tcp_stream(&stream2)?;
        let connector2 = TcpConnector { stream: Some(stream2) };
        channel2 = Endpoint::from_shared(format!("https://{}", server_addrs[1]).to_string()).expect("Invalid URL").connect_with_connector_lazy(connector2);

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(ptimer)).await;
                println!("Checking health of connection");
                
                let stats = get_tcp_stats(&stream_for_stats).unwrap();
                let stats2 = get_tcp_stats(&stream2_for_stats).unwrap();

                let stats = match get_tcp_stats(&stream_for_stats) {
                    Some(stats) => {
                        println!("Primary RTT: {} us", stats.rtt_us);
                        stats
                    }
                    None => {
                        println!("Failed to get TCP stats of primary path");
                        panic!("Failed to get TCP stats of primary path")
                    }
                };

                let stats2 = match get_tcp_stats(&stream2_for_stats) {
                    Some(stats) => {
                        println!("Backup RTT: {} us", stats.rtt_us);
                        stats
                    }
                    None => {
                        println!("Failed to get TCP stats of backup path");
                        panic!("Failed to get TCP stats of backup path")
                    }
                };
               
                if stats.rtt_us > stats2.rtt_us {
                    println!("Switching to backup connection");
                    change_channel_clone.store(true, SeqCst);
                } else {
                    println!("Using primary connection");
                    change_channel_clone.store(false, SeqCst);
                }
                
            
            }
        });


    } else {
        println!("Using single server address");
        let addr = "127.0.0.1:3434".parse().unwrap();

        let socket = TcpSocket::new_v4()?;
        socket.bind(addr)?;

        //let listener = socket.listen(1024)?;

        //let stream = TcpStream::connect(server_addr.clone()).await?;
        let stream = socket.connect(server_addr.clone().parse().expect("Problem parsing the server adresse.")).await?;
        let stream_for_stats = clone_tokio_tcp_stream(&stream)?;
        let connector = TcpConnector { stream: Some(stream) };
        channel = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL").connect_with_connector_lazy(connector);

        /*
        let stream2 = TcpStream::connect(server_addr.clone()).await?;
        let stream2_for_stats = clone_tokio_tcp_stream(&stream)?;
        let connector2 = TcpConnector { stream: Some(stream) };
        channel2 = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL").connect_with_connector_lazy(connector);*/

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_millis(ptimer)).await;
                println!("Checking health of connection");
    
                match get_tcp_stats(&stream_for_stats) {
                    Some(stats) => {
                        println!("RTT: {} us", stats.rtt_us);
                        println!("RTT variance: {} us", stats.rtt_var_us);
                        println!("Retransmits: {}", stats.retransmits);
                    }
                    None => {
                        println!("Failed to get TCP stats");
                    }
                }
            }
        });

        /*
        channel = Channel::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
        .tls_config(tls)?
        .connect_lazy();
        */
        channel2 = channel.clone();


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
        if monitoring {
            let mut client1 = FileServiceClient::new(channel);
            let mut client2 = FileServiceClient::new(channel2);

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

                    let used_client;

                    if change_channel.load(SeqCst) {
                        println!("** Using backup connection **");
                        used_client = &mut client2;
                    } else {
                        println!("** Using primary connection **");
                        used_client = &mut client1;
                    }
                    
                    response = match timeout(Duration::from_millis(rtime), used_client.exchange_file(request)).await {
                        Ok(response) => {retry = false; response?.into_inner()},
                        Err(_) => {
                            println!("Cancelled request after 1s");
                            problem.swap(true, SeqCst);
                            continue;
                        }
                    };

                    let mut new_file = File::create("downloaded_example").await?;
                    new_file.write_all(&response.data).await?;
                }

            
                println!("File exchanged successfully!");

                let d = s.elapsed();
                let c = start.elapsed();
                println!("Time elapsed: ({:?} ,{:?})", c.as_secs_f64(),d.as_secs_f64());
            }

        } else {
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
                    /*
                    response = match timeout(Duration::from_millis(rtime), client.exchange_file(request)).await {
                        Ok(response) => {retry = false; response?.into_inner()},
                        Err(_) => {
                            println!("Cancelled request after 1s");
                            problem.swap(true, SeqCst);
                            continue;
                        }
                    };*/


                    response = client.exchange_file(request).await?.into_inner();
                    retry = false;

                    let mut new_file = File::create("downloaded_example").await?;
                    new_file.write_all(&response.data).await?;
                }

            
                println!("File exchanged successfully!");

                let d = s.elapsed();
                let c = start.elapsed();
                println!("Time elapsed: ({:?} ,{:?})", c.as_secs_f64(),d.as_secs_f64());
            }
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

