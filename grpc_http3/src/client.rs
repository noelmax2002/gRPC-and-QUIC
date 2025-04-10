use std::net::{SocketAddr, SocketAddrV4};

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use echo::{echo_client::EchoClient, EchoRequest};
use filetransfer::file_service_client::FileServiceClient;
use filetransfer::{FileData, FileRequest};

use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Endpoint,Uri};
use tower::service_fn;
use hyper_util::rt::tokio::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc;
use tokio::task;
use log::{info, error, debug};
use tokio::time::sleep;
use std::time::{Duration, Instant};
use docopt::Docopt;
use tonic::transport::Channel;
use tokio::fs::{self,File};
use std::collections::HashMap;
use tokio::task::JoinHandle;
use slab::Slab;
use log::{trace,warn};
use std::net::{IpAddr, Ipv4Addr};
use quiche::{PathId, CIDSeq};

use quiche::h3::NameValue;

use ring::rand::*;
use rand::seq::SliceRandom;
use rand::thread_rng;

const MAX_DATAGRAM_SIZE: usize = 1350;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

pub mod echo {
    tonic::include_proto!("echo");
}

pub mod filetransfer {
    tonic::include_proto!("filetransfer");
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// Write the Docopt usage string.
const USAGE: &'static str = "
Usage: client [options]

Options:
    -s --sip ADDRESS                Server IPv4 address and port [default: 127.0.0.1:4433].
    -A --address ADDR ...           Client potential multiple addresses.
    -p --proto PROTOCOL             Choose the protoBuf to use [default: helloworld].
    --timeout TIMEOUT               Idle timeout of the QUIC connection in milliseconds [default: 5000].
    -e --early                      Enable sending early data.
    -r --connetion-resumption       Enable connection resumption.
    --poll-timeout TIMEOUT          Timeout for polling the event loop in milliseconds [default: 1].
    --file FILE                     File to upload [default: ../swift_file_examples/small.txt].
    --multipath                     Enable multipath.
    --scheduler SCHEDULER           Choose the scheduler to use [default: lrtt].
    -n --num NUM                    Number of requests to send [default: 10].
    -t --time DURATION              Duration between each request [default: 500].
    --ack-eliciting-timer TIMEOUT    Timeout for the ack elicting timer in milliseconds [default: 100].
    --rcvd-threshold TIMEOUT        Timeout for the rcvd threshold in milliseconds [default: 1000].
    --nocapture                     Do not capture the output of the test.
    
";

struct PartialRequest{
    body: Vec<u8>,

    written: usize,
}

#[tokio::main]
async fn main() -> Result<()> {

    //Read from CLI to learn the server/client address.
    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let mut server_addr = args.get_str("--sip").to_string();
    let proto = args.get_str("--proto").to_string();
    let mut file_path = args.get_str("--file").to_string();
    let mut n = args.get_str("--num").parse().unwrap();
    let dur = args.get_str("--time").parse::<u64>().unwrap();

    //127.0.0.1:4433
    //192.168.1.7:8080"

    let channel = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
        .connect_with_connector_lazy(service_fn(|uri: Uri| async {
            let (client, server) = tokio::io::duplex(1_000_000);
            task::spawn(async move {
                let (to_client, from_io) = mpsc::channel(1000);
                let (to_io, from_client) = mpsc::channel(1000);

                let mut new_client = Client {
                    stream: client,
                    to_io: to_io,
                    from_io: from_io,
                };
            
                // Let the client run on its own.
                tokio::spawn(async move {
                    new_client.run().await.unwrap();
                });

                run_client(uri, to_client, from_client).await.unwrap();
            });

            Ok::<_, std::io::Error>(TokioIo::new(server))
    }));

    let start = Instant::now();

    if proto.as_str() == "echo" {
        let mut client = EchoClient::new(channel);
        bidirectional_streaming_echo(&mut client, 100).await;
        /*
        let mut client = EchoClient::new(channel);
        let mut client2 = client.clone();

        let join_handle: task::JoinHandle<_> = task::spawn(async move {
            bidirectional_streaming_echo(&mut client, 100).await;
        });

        let join_handle2: task::JoinHandle<_> = task::spawn(async move{
            bidirectional_streaming_echo(&mut client2, 100).await;
        });

        join_handle.await?;
        join_handle2.await?; 
        */
        


    } else if proto.as_str() == "helloworld" {
        let mut client = GreeterClient::new(channel);

        let request = tonic::Request::new(HelloRequest {
            name: "aaaaaa".into(),
        });
        
        
        let response = client.say_hello(request).await.unwrap();
        println!("response: {:?}", response.get_ref().message);

        let request = tonic::Request::new(HelloRequest {
            name: "Guillaume".into(),
        });
        
        let response = client.say_hello(request).await.unwrap();
        println!("response: {:?}", response.get_ref().message);

        let request = tonic::Request::new(HelloRequest {
            name: "Maelle".into(),
        });

        let response = client.say_hello(request).await.unwrap();
        println!("response: {:?}", response.get_ref().message);

        let request = tonic::Request::new(HelloRequest {
            name: "Theo".into(),
        });
    
        let response = client.say_hello(request).await.unwrap();
        println!("response: {:?}", response.get_ref().message);

        let request = tonic::Request::new(HelloRequest {
            name: "Mathias".into(),
        });
    
        let response = client.say_hello(request).await.unwrap();
        println!("response: {:?}", response.get_ref().message);

        let request = tonic::Request::new(HelloRequest {
            name: "Chloe".into(),
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
        let mut client2 = client.clone();

        let join_handle: task::JoinHandle<_> = task::spawn(async move {
            let mut file = File::open("./swift_file_examples/other.txt").await;
            let mut buffer = Vec::new();
            file.expect("failed to open the file").read_to_end(&mut buffer).await;

            let request = tonic::Request::new(FileData {
                filename: "uploaded_example".into(),
                data: buffer,
            });

            let response = client.upload_file(request).await.expect("Fail on the first file").into_inner();
            println!("Upload Response: {}", response.message);
        });

        let join_handle2: task::JoinHandle<_> = task::spawn(async move{
            let mut file = File::open(file_path).await;
            let mut buffer = Vec::new();
            file.expect("Failed to open the file").read_to_end(&mut buffer).await;

            let request = tonic::Request::new(FileData {
                filename: "uploaded_example".into(),
                data: buffer,
            });

            let response = client2.upload_file(request).await.expect("Fail on the second file").into_inner();
            println!("Upload Response: {}", response.message);
        });

        join_handle.await?;
        join_handle2.await?; 

     */
        /* 
        // Download a file
        let request = tonic::Request::new(FileRequest {
            filename: "uploaded_example".into(),
        });

        let response = client.download_file(request).await?.into_inner();
        let mut new_file = File::create("downloaded_example").await?;
        new_file.write_all(&response.data).await?;

        println!("File downloaded successfully!"); 
        */
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
            println!("Time elapsed: ({:?} ,{:?})", c.as_secs_f64(), d.as_secs_f64());
        }

    } else {
        panic!("Invalid protoBuf");
    }
        
    let duration = start.elapsed();
    println!("Total time elapsed: {:?}", duration.as_secs_f64());

    Ok(())
}

pub async fn run_client(uri: Uri, to_client: Sender<Vec<u8>>, mut from_client: Receiver<Vec<u8>>,) -> Result<()> {
    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let idle_timeout = args.get_str("--timeout").parse::<u64>().unwrap();
    let early_data = args.get_bool("--early");
    let connection_resumption = args.get_bool("--connection-resumption");
    let poll_timeout = args.get_str("--poll-timeout").parse::<u64>().unwrap();
    let multipath = args.get_bool("--multipath");
    let scheduler = args.get_str("--scheduler");
    let ack_eliciting_timer = args.get_str("--ack-eliciting-timer").parse::<u64>().unwrap();
    let rcvd_threshold = args.get_str("--rcvd-threshold").parse::<u64>().unwrap();
    
    loop { 
        // ----- QUIC Connection -----
        let mut buf = [0; 65535];
        let mut out = [0; MAX_DATAGRAM_SIZE];
        let session_file = "session_file.bin";
        let mut sending_request = false;
        let mut stream_id = 0;

        // Setup the event loop.
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);

        let peer_addr = SocketAddr::V4(SocketAddrV4::new(uri.host().unwrap().parse().unwrap(), uri.port().unwrap().into()));

        //creates a vectors or local addresses 'SocketAddr' with 127.0.0.1:0 and 127.0.0.1:4455

        //let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        //let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 3355);

        let mut addrs: Vec<SocketAddr> = args
            .get_vec("--address")
            .into_iter()
            .filter_map(|a| a.parse().ok())
            .collect();

        if addrs.is_empty() {
            addrs.push(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080));
            addrs.push(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 3355));
        }
            
        
        // Create the UDP socket backing the QUIC connection, and register it with
        // the event loop.
        let (sockets, src_addr_to_token, local_addr) = create_sockets(&mut poll, &peer_addr, addrs);
        let mut addrs = Vec::with_capacity(sockets.len());
        addrs.push(local_addr);
        for src in src_addr_to_token.keys() {
            if *src != local_addr {
                addrs.push(*src);
            }
        }
        //println!("Addresses : {:?}", addrs);

        let mut rm_addrs: Vec<(std::time::Duration, SocketAddr)> = Vec::new();
        let mut status: Vec<(std::time::Duration, SocketAddr, bool)> = Vec::new();
        let mut retire_dcids: Vec<(std::time::Duration, PathId, CIDSeq)> = Vec::new();

        //status.push(((std::time::Duration::from_millis(1)), addr2, true));

        // Create the configuration for the QUIC connection.
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

        // *CAUTION*: this should not be set to `false` in production!!!
        config.verify_peer(false);

        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();


        config.set_max_idle_timeout(idle_timeout);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100_000);
        config.set_initial_max_streams_uni(100_000);
        config.set_disable_active_migration(false);
        if early_data {
            config.enable_early_data();
        }
        
        if multipath {
            config.set_initial_max_path_id(100);
        } else {
            config.set_initial_max_path_id(0);
        }

        config.set_max_connection_window(1_000_000);
        config.set_max_stream_window(1_000_000);

        let mut http3_conn: Option<quiche::h3::Connection> = None;

        // Generate a random source connection ID for the connection.
        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        SystemRandom::new().fill(&mut scid[..]).unwrap();

        let scid = quiche::ConnectionId::from_ref(&scid);

        let rng: SystemRandom = SystemRandom::new();


        // Get local address.
        //let local_addr = socket.local_addr().unwrap();

        // Create a QUIC connection and initiate handshake.
        let mut conn =
            quiche::connect(None, &scid, local_addr ,peer_addr, &mut config)
                .unwrap();

            
        if let Ok(session) = std::fs::read(session_file) {
            conn.set_session(&session).ok();
        }
            

        debug!(
            "connecting to {:} from {:} with scid {}",
            peer_addr,
            local_addr,
            hex_dump(&scid)
        );

        let (write, send_info) = conn.send(&mut out).expect("initial send failed");
        let token = src_addr_to_token[&send_info.from];

        while let Err(e) = sockets[token].send_to(&out[..write], send_info.to) {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                debug!("send() would block");
                continue;
            }

            panic!("send() failed: {:?}", e);
        }

        debug!("written {}", write);

        let h3_config = quiche::h3::Config::new().unwrap();

        // Prepare request. (dummy request for now)
        let req = vec![
            quiche::h3::Header::new(b":method", b"POST"),
        ];

        let mut waiting_to_be_sent: Vec<PartialRequest> = Vec::new();

        let app_data_start = std::time::Instant::now();

        let mut send_time_map: HashMap<std::net::SocketAddr, Duration>  = HashMap::new();
        let mut rcvd_time_map: HashMap<std::net::SocketAddr, Duration>  = HashMap::new();

        for addr in addrs.iter() {
            send_time_map.insert(*addr, app_data_start.elapsed());
            rcvd_time_map.insert(*addr, app_data_start.elapsed());
        }

        let mut probed_paths = 0;

        let mut scid_sent = false;
        let mut new_path_probed = false;

        loop {     
            if !conn.is_in_early_data() {
                if poll_timeout == 0 {
                    poll.poll(&mut events, conn.timeout()).unwrap();
                }
                poll.poll(&mut events, Some(Duration::from_millis(poll_timeout))).unwrap();
            }
            //sleep(Duration::from_millis(1)).await;

            // If the event loop reported no events, it means that the timeout
            // has expired, so handle it without attempting to read packets. We
            // will then proceed with the send loop.
            if events.is_empty() {

                conn.on_timeout();
            }

            for event in &events {
                let token = event.token().into();
                let socket = &sockets[token];
                let local_addr = socket.local_addr().unwrap();
                // Read incoming UDP packets from the socket and feed them to quiche,
                // until there are no more packets to read.
                'read: loop {
                    let (len, from) = match socket.recv_from(&mut buf) {
                        Ok(v) => v,

                        Err(e) => {
                            // There are no more UDP packets to read, so end the read
                            // loop.
                            if e.kind() == std::io::ErrorKind::WouldBlock {
                                debug!("recv() would block");
                                break 'read;
                            } 

                            info!("recv() failed: {:?}", e);
                            break;  

                            //panic!("recv() failed: {:?}", e);
                        },
                    };

                    info!("got {} bytes on addr {}", len, local_addr);

                    let recv_info = quiche::RecvInfo {
                        to: local_addr,
                        from,
                    };

                    // Process potentially coalesced packets.
                    let read = match conn.recv(&mut buf[..len], recv_info) {
                        Ok(v) => v,

                        Err(e) => {
                            error!("recv failed: {:?}", e);
                            continue 'read;
                        },
                    };

                    rcvd_time_map.insert(local_addr, app_data_start.elapsed());

                    info!("processed {} bytes", read);
                }
            }

            if conn.is_closed() {
                //println!("connection closed, {:?}", conn.stats());
                //println!("gRPC is disconnected: {:?}", from_client.is_closed());

                if !conn.is_established() {
                    error!(
                        "Handshake failed",
                    );
                }
                
                if let Some(session) = conn.session() {
                    std::fs::write(session_file, &session).ok();
                }

                if from_client.is_closed() {
                    println!("Connection to client IO is closed");
                    return Ok(());
                } else {
                    //Begin again the connection
                    if connection_resumption {
                        break;
                    } else {
                        return Ok(());
                    }
                }
                //return Ok(());
            }

            // Create a new HTTP/3 connection once the QUIC connection is established.
            if (conn.is_established() || conn.is_in_early_data()) && http3_conn.is_none() {
                http3_conn = Some(
                    quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                    .expect("Unable to create HTTP/3 connection, check the server's uni stream limit and window size"),
                );
            }

            // Send HTTP requests once the QUIC connection is established, and until
            // all requests have been sent.
            if let Some(h3_conn) = &mut http3_conn {
                loop{
                    waiting_to_be_sent.retain_mut(|request| {
                        if !sending_request {                     
                            stream_id = match h3_conn.send_request(&mut conn, &req, false) {
                                Ok(v) => v,
                        
                                Err(quiche::h3::Error::StreamBlocked) => {
                                    //println!("stream blocked");
                                    return true;
                                },
                        
                                Err(e) => {
                                    error!("{} stream send failed {:?}", conn.trace_id(), e);
                                    return true;
                                },
                            };
                        }

                        let body = &request.body[request.written..];

                        let mut end = false;
                        let len = body.len();
                        if len >= 9 && body[len - 9..len-1] == [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00] {
                            end = true;
                        }

                        let written = match h3_conn.send_body(&mut conn, stream_id, body, end) {
                            Ok(v) => v,
                    
                            Err(quiche::h3::Error::Done) => 0,
                    
                            Err(e) => {
                                error!("{} stream send failed {:?}", conn.trace_id(), e);
                                return false;
                            },
                        };
                    
                        request.written += written;
                    
                        if request.written == request.body.len() {
                            if end {
                                //println!("-END of gRPC request data-");
                                sending_request = false;
                            }

                            return false;
                        }
                    
                        true
                    });

                    /*
                    let mut i = 0;
                    for request in waiting_to_be_sent.iter_mut() {
                        let stream_id = match h3_conn.send_request(&mut conn, &req, false) {
                            Ok(v) => v,
                    
                            Err(quiche::h3::Error::StreamBlocked) => {
                                println!("stream blocked");
                                break;
                            },
                    
                            Err(e) => {
                                error!("{} stream send failed {:?}", conn.trace_id(), e);
                                break;
                            },
                        };
                    
                        let written = match h3_conn.send_body(&mut conn, stream_id, &request.body[request.written..], false) {
                            Ok(v) => v,
                    
                            Err(quiche::h3::Error::Done) => 0,
                    
                            Err(e) => {
                                error!("{} stream send failed {:?}", conn.trace_id(), e);
                                break;
                            },
                        };
                    
                        request.written += written;
                    
                        if request.written == request.body.len() {
                            waiting_to_be_sent.remove(i);
                            continue;
                        }
                    
                        i += 1;
                    } */
                        

                    let data = match from_client.try_recv() {
                        Ok(v) => {
                            v },
                        Err(e) => {
                            if e == mpsc::error::TryRecvError::Empty {
                                sleep(Duration::from_millis(10)).await;
                                //sleep(Duration::new(0,1)).await;
                                break;
                            }

                            //println!("recv() failed: {:?}", e);
                            return Ok(());
                        },
                    };

                    //println!("sending HTTP request {:?}", req);
                    //println!("sending HTTP data of size {:?}", data.len());
                    //println!("{:?}", data);
                    //let stream_id = h3_conn.send_request(&mut conn, &req, false).unwrap();
                    //println!("stream_id: {:?}", stream_id);
                    //h3_conn.send_body(&mut conn, stream_id, &data, true).unwrap();

                    //println!("sending HTTP data of size {:?}", data.len());
                    if !sending_request {
                        stream_id = match h3_conn.send_request(&mut conn, &req, false) {
                            Ok(v) => v,
                    
                            Err(quiche::h3::Error::StreamBlocked) => {
                                info!("stream blocked");
                                let request = PartialRequest {
                                    body: data,
                                    written: 0,
                                };
                    
                                waiting_to_be_sent.push(request);
                                break;
                            },
                    
                            Err(e) => {
                                error!("{} stream send failed {:?}", conn.trace_id(), e);
                                break;
                            },
                        };
                        sending_request = true;
                    }

                    //println!("stream_id: {:?}", stream_id);
                    //let vec_to_string = String::from_utf8_lossy(&data);
                    //println!("{:?}", data);
                    //println!("{}", vec_to_string);
                    let mut end = false;
                    let len = data.len();
                    if len >= 9 && data[len - 9..len-1] == [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00] {
                        end = true;
                    }

                    let written = match h3_conn.send_body(&mut conn, stream_id, &data, end) {
                        Ok(v) => v,
                
                        Err(quiche::h3::Error::Done) => 0,
                
                        Err(e) => {
                            error!("{} stream send failed {:?}", conn.trace_id(), e);
                            break;
                        },
                    };
                
                    if written < data.len() {
                        let request = PartialRequest {
                            body: data,
                            written,
                        };
                
                        waiting_to_be_sent.push(request);
                    } else {
                        if written >= 9 && data[data.len() - 9..data.len()-1] == [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00] { // end of gRPC request data
                            //println!("-END of gRPC request data-");
                            sending_request = false;
                        }
                    }


                }
            }

            // force frames to be sent (if nothing then its PING frames) - to get correct RTT-measurements on all paths
            if(conn.is_established()){
                for (addr, time) in send_time_map.iter(){
                    if app_data_start.elapsed() - *time > std::time::Duration::from_millis(ack_eliciting_timer) {
                        conn.send_ack_eliciting_on_path(*addr, peer_addr).ok();
                    }
                }
            }


            if let Some(http3_conn) = &mut http3_conn {            
                // Process HTTP/3 events.
                loop {
                    match http3_conn.poll(&mut conn) {
                        Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                            info!(
                                "got response headers {:?} on stream id {}",
                                hdrs_to_strings(&list),
                                stream_id
                            );
                        },

                        Ok((stream_id, quiche::h3::Event::Data)) => {
                            while let Ok(read) =
                                http3_conn.recv_body(&mut conn, stream_id, &mut buf)
                            {
                                info!(
                                    "got {} bytes of response data on stream {}",
                                    read, stream_id
                                );
                                if to_client.is_closed() {
                                    return Ok(());
                                }
                                to_client.send(buf[..read].to_vec()).await?;
                                sleep(Duration::new(0,1)).await;
                            }
                        },

                        Ok((_stream_id, quiche::h3::Event::Finished)) => {
                            info!(
                                "response received, closing...",
                            );

                            conn.close(true, 0x100, b"kthxbye").unwrap();
                        },

                        Ok((_stream_id, quiche::h3::Event::Reset(e))) => {
                            error!(
                                "request was reset by peer with {}, closing...",
                                e
                            );

                            conn.close(true, 0x100, b"kthxbye").unwrap();
                        },

                        Ok((goaway_id, quiche::h3::Event::GoAway)) => {
                            debug!("GOAWAY id={}", goaway_id);
                        },

                        //Ok((_, quiche::h3::Event::Datagram)) => (),

                        Ok((_, quiche::h3::Event::PriorityUpdate)) => (),

                        Err(quiche::h3::Error::Done) => {
                            break;
                        },

                        Err(e) => {
                            error!("HTTP/3 processing failed: {:?}", e);

                            break;
                        },
                    }
                }
            }

            // Handle path events.
            while let Some((pid, qe)) = conn.path_event_next() {
                match qe {
                    quiche::PathEvent::New(..) => unreachable!(),

                    quiche::PathEvent::Validated(local_addr, peer_addr) => {
                        println!(
                            "Path ({}, {}) with ID {} is now validated",
                            local_addr, peer_addr, pid
                        );
                        if conn.is_multipath_enabled() {
                            conn.set_active(local_addr, peer_addr, true).ok();
                        }
                        /* else if args.perform_migration {
                            conn.migrate(local_addr, peer_addr).unwrap();
                            migrated = true;
                        }*/
                    },

                    quiche::PathEvent::FailedValidation(local_addr, peer_addr) => {
                        println!(
                            "Path ({}, {}) with ID {} failed validation",
                            local_addr, peer_addr, pid
                        );
                    },

                    quiche::PathEvent::Closed(local_addr, peer_addr, e) => {
                        println!(
                            "Path ({}, {}) with ID {} is now closed and unusable; err = {}",
                            local_addr, peer_addr, pid, e,
                        );
                    },

                    quiche::PathEvent::ReusedSourceConnectionId(
                        cid_seq,
                        old,
                        new,
                    ) => {
                        println!(
                            "Peer reused cid seq {} (initially {:?}) on {:?}, path ID {}",
                            cid_seq, old, new, pid
                        );
                    },

                    quiche::PathEvent::PeerMigrated(..) => unreachable!(),

                    quiche::PathEvent::PeerPathStatus(..) => {},
                }
            }

            // See whether source Connection IDs have been retired.
            while let Some((path_id, retired_scid)) = conn.retired_scid_on_path_next()
            {
                info!(
                    "Retiring source CID {:?} from path_id {}",
                    retired_scid, path_id
                );
            
            }

            // Provides as many CIDs as possible.
            for path_id in conn.path_ids() {
                while conn.scids_left_on_path(path_id) > 0 {
                    let (scid, reset_token) = generate_cid_and_reset_token(&rng);

                    if conn
                        .new_scid_on_path(path_id, &scid, reset_token, false)
                        .is_err()
                    {
                        break;
                    }

                    scid_sent = true;
                }
            }

            if probed_paths < addrs.len() &&
            conn.available_dcids() > 0 &&
            conn.probe_path(addrs[probed_paths], peer_addr).is_ok()
            {
                println!("Probing path {}", addrs[probed_paths]);
                probed_paths += 1;
            }   

            if conn.is_multipath_enabled() {
                rm_addrs.retain(|(d, addr)| {
                    if app_data_start.elapsed() >= *d {
                        println!("Abandoning path {:?}", addr);
                        conn.abandon_path(*addr, peer_addr, 0).is_err()
                    } else {
                        true
                    }
                });
    
                status.retain(|(d, addr, available)| {
                    if app_data_start.elapsed() >= *d {
                        let status = (*available).into();
                        println!("Advertising path status {status:?} to {addr:?}");
                        conn.set_path_status(*addr, peer_addr, status, true)
                            .is_err()
                    } else {
                        true
                    }
                });
            }
    
            if conn.is_established() {
                retire_dcids.retain(|(d, path_id, cid_seq)| {
                    if conn.available_dcids_on_path(*path_id) > 0 && app_data_start.elapsed() >= *d {
                        info!("retiring DCID sequence number {cid_seq} with path_id {path_id}");
                        if let Err(e) = conn.retire_dcid_on_path(*path_id, *cid_seq) {
                            error!("error when retiring DCID: {e:?}");
                        }
                        false
                    } else {
                        true
                    }
                });
            }

            // Determine in which order we are going to iterate over paths.
            //let scheduled_tuples = lowest_latency_scheduler(&conn);
            let mut scheduled_tuples = scheduler_fn(&conn, scheduler, &rcvd_time_map, &app_data_start, rcvd_threshold);


            // Generate outgoing QUIC packets and send them on the UDP socket, until
            // quiche reports that there are no more packets to be sent.
            for (local_addr, peer_addr) in scheduled_tuples {
                let token = src_addr_to_token[&local_addr];
                let socket = &sockets[token];
                info!(
                    "sending on path ({}, {})",
                    local_addr, peer_addr
                );
                loop {
                    let (write, send_info) = match conn.send_on_path(
                        &mut out,
                        Some(local_addr),
                        Some(peer_addr),
                    ) {
                        Ok(v) => v,

                        Err(quiche::Error::Done) => {
                            //println!("{} -> {}: done writing", local_addr, peer_addr);
                            break;
                        },

                        Err(e) => {
                            info!(
                                "{} -> {}: send failed: {:?}",
                                local_addr, peer_addr, e
                            );

                            conn.close(false, 0x1, b"fail").ok();
                            break;
                        },
                    };

                    if let Err(e) = socket.send_to(&out[..write], send_info.to) {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            println!(
                                "{} -> {}: send() would block",
                                local_addr,
                                send_info.to
                            );
                            break;
                        }

                        panic!("send() failed: {:?}", e);
                    }

                    send_time_map.insert(local_addr, app_data_start.elapsed());

                    info!("{} -> {}: written {}", local_addr, send_info.to, write);
                }
            }
    

            /* 
            // Generate outgoing QUIC packets and send them on the UDP socket, until
            // quiche reports that there are no more packets to be sent.
            loop {
                let (write, send_info) = match conn.send(&mut out) {
                    Ok(v) => v,

                    Err(quiche::Error::Done) => {
                        break;
                    },

                    Err(e) => {
                        error!("send failed: {:?}", e);

                        conn.close(false, 0x1, b"fail").ok();
                        break;
                    },
                };

                if let Err(e) = socket.send_to(&out[..write], send_info.to) {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("send() would block");
                        break;
                    }

                    panic!("send() failed: {:?}", e);
                }

                debug!("written {}", write);
            }
            */
            if conn.is_closed() {
                //println!("connection closed, {:?}", conn.stats());
                //println!("gRPC is disconnected: {:?}", from_client.is_closed());

                if !conn.is_established() {
                   error!(
                        "Handshake failed",
                    );
                }    

                if let Some(session) = conn.session() {
                    std::fs::write(session_file, &session).ok();
                }

                if from_client.is_closed() {
                    return Ok(());
                } else {
                    //Begin again the connection
                    if connection_resumption {
                        break;
                    } else {
                        return Ok(());
                    }
                }
                //return Ok(());
            }
        }
        fs::remove_file("./session_file.bin").await?;
    }
}

fn create_sockets(
    poll: &mut mio::Poll, peer_addr: &std::net::SocketAddr, addrs: Vec<SocketAddr>,
) -> (
    Slab<mio::net::UdpSocket>,
    HashMap<std::net::SocketAddr, usize>,
    std::net::SocketAddr,
) {
    let mut sockets = Slab::with_capacity(std::cmp::max(addrs.len(), 1));
    let mut src_addrs = HashMap::new();
    let mut first_local_addr = None;

    // Create UDP sockets backing the QUIC connection, and register them with
    // the event loop. Check first user-provided addresses and keep the ones
    // compatible with the address family of the peer.
    for src_addr in addrs.iter().filter(|sa| {
        (sa.is_ipv4() && peer_addr.is_ipv4()) ||
            (sa.is_ipv6() && peer_addr.is_ipv6())
    }) {
        let socket = mio::net::UdpSocket::bind(*src_addr).unwrap();
        let local_addr = socket.local_addr().unwrap();
        let token = sockets.insert(socket);
        src_addrs.insert(local_addr, token);
        poll.registry()
            .register(
                &mut sockets[token],
                mio::Token(token),
                mio::Interest::READABLE,
            )
            .unwrap();
        if first_local_addr.is_none() {
            first_local_addr = Some(local_addr);
        }
    }

    // If there is no such address, rely on the default INADDR_IN or IN6ADDR_ANY
    // depending on the IP family of the server address. This is needed on macOS
    // and BSD variants that don't support binding to IN6ADDR_ANY for both v4
    // and v6.
    if first_local_addr.is_none() {
        let bind_addr = match peer_addr {
            std::net::SocketAddr::V4(_) => "0.0.0.0:0",
            std::net::SocketAddr::V6(_) => "[::]:0",
        };
        let bind_addr = bind_addr.parse().unwrap();
        let socket = mio::net::UdpSocket::bind(bind_addr).unwrap();
        let local_addr = socket.local_addr().unwrap();
        let token = sockets.insert(socket);
        src_addrs.insert(local_addr, token);
        poll.registry()
            .register(
                &mut sockets[token],
                mio::Token(token),
                mio::Interest::READABLE,
            )
            .unwrap();
        first_local_addr = Some(local_addr)
    }

    (sockets, src_addrs, first_local_addr.unwrap())
}

/// Generate a new pair of Source Connection ID and reset token.
pub fn generate_cid_and_reset_token<T: SecureRandom>(
    rng: &T,
) -> (quiche::ConnectionId<'static>, u128) {
    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    rng.fill(&mut scid).unwrap();
    let scid = scid.to_vec().into();
    let mut reset_token = [0; 16];
    rng.fill(&mut reset_token).unwrap();
    let reset_token = u128::from_be_bytes(reset_token);
    (scid, reset_token)
}

/// Generate a ordered list of 4-tuples on which the host should send packets,
/// following a lowest-latency scheduling.
fn scheduler_fn(
    conn: &quiche::Connection,
    scheduler: &str,
    rcvd_time_map: &HashMap<std::net::SocketAddr, Duration>,
    start: &std::time::Instant,
    threshold: u64,
) -> impl Iterator<Item = (std::net::SocketAddr, std::net::SocketAddr)> {
    if scheduler == "lrtt" {
        //lowest-rtt-first scheduler
        use itertools::Itertools;
        let mut paths: Vec<_> = conn
            .path_stats()
            .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
            .filter(|p| {
                if let Some(time) = rcvd_time_map.get(&p.local_addr) {
                    let n = start.elapsed() - *time;
                    info!("path {:?} with threshold : {:?}", p.path_id, n);
                    n <= std::time::Duration::from_millis(threshold)
                } else {
                    true
                }
            })
            .sorted_by_key(|p| {info!("path {:?} with RTT : {:?}", p, p.rtt); p.rtt})
            .map(|p| (p.local_addr, p.peer_addr))
            .collect();

        paths.into_iter()
    } else {
        //random scheduler
        use itertools::Itertools;

        let mut paths: Vec<_> = conn
            .path_stats()
            .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
            .map(|p| {info!("path {:?} with RTT : {:?}", p.path_id, p.rtt); (p.local_addr, p.peer_addr)})
            .collect();   

        paths.shuffle(&mut thread_rng());

        paths.into_iter()
    }
    
}



pub struct Client {
    pub stream: DuplexStream,
    pub to_io: Sender<Vec<u8>>,
    pub from_io: Receiver<Vec<u8>>,
}

impl Client {
    // Handle the communication between Quic (synchronous) thread and gRPC (asynchronous) thread.
    pub async fn run(&mut self) -> Result<()> {
        let mut buf = [0u8; 1500];
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => {
                    if len == 0 {
                        return Ok(());
                    }

                    self.handle_grpc_msg(&buf[..len]).await?
                },
            }

            if self.from_io.is_closed() {
                return Ok(());
            }
        }
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        //println!("Received IO message: {:?}", msg);
        /* 
        println!("[CLIENT] Received IO message of size: {:?}", msg.len());
        let vec_to_string = String::from_utf8_lossy(&msg);
        println!("{:?}", msg);
        println!("{}", vec_to_string);
        */
        self.stream.write(&msg).await?;

        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        //println!("Received gRPC message: {:?}", msg);
        //println!("Size of msg : {:?}", msg.len());
        if self.to_io.is_closed() {
            return Ok(());
        }
        
        self.to_io.send(msg.to_vec()).await?;

        Ok(())
    }
}

fn hex_dump(buf: &[u8]) -> String {
    let vec: Vec<String> = buf.iter().map(|b| format!("{b:02x}")).collect();

    vec.join("")
}

pub fn hdrs_to_strings(hdrs: &[quiche::h3::Header]) -> Vec<(String, String)> {
    hdrs.iter()
        .map(|h| {
            let name = String::from_utf8_lossy(h.name()).to_string();
            let value = String::from_utf8_lossy(h.value()).to_string();

            (name, value)
        })
        .collect()
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

