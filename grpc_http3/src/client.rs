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


use quiche::h3::NameValue;

use ring::rand::*;

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
    -p --proto PROTOCOL             Choose the protoBuf to use [default: helloworld].
    -t --timeout TIMEOUT            Idle timeout of the QUIC connection in milliseconds [default: 5000].
    -e --early                      Enable sending early data.
    -r --connetion-resumption       Enable connection resumption.
    --poll-timeout TIMEOUT          Timeout for polling the event loop in milliseconds [default: 1].
    --file FILE                     File to upload [default: ../swift_file_examples/small.txt].
    --nocapture                     Do not capture the output of the test.
";

#[tokio::main]
async fn main() -> Result<()> {

    //Read from CLI to learn the server/client address.
    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let mut server_addr = args.get_str("--sip").to_string();
    let proto = args.get_str("--proto").to_string();
    let file_path = args.get_str("--file").to_string();

    //127.0.0.1:4433
    //192.168.1.7:8080"

    let channel = Endpoint::from_shared(format!("https://{}", server_addr).to_string()).expect("Invalid URL")
        .connect_with_connector_lazy(service_fn(|uri: Uri| async {
            let (client, server) = tokio::io::duplex(12000);
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
        let mut file = File::open(file_path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        let request = tonic::Request::new(FileData {
            filename: "uploaded_example".into(),
            data: buffer,
        });

        let response = client.upload_file(request).await?.into_inner();
        //println!("Upload Response: {}", response.message);

        // Download a file
        let request = tonic::Request::new(FileRequest {
            filename: "uploaded_example".into(),
        });

        let response = client.download_file(request).await?.into_inner();
        let mut new_file = File::create("downloaded_example").await?;
        new_file.write_all(&response.data).await?;

        //println!("File downloaded successfully!");

    } else {
        panic!("Invalid protoBuf");
    }
        
    let duration = start.elapsed();
    println!("Time elapsed: {:?}", duration.as_secs_f64());

    Ok(())
}

pub async fn run_client(uri: Uri, to_client: Sender<Vec<u8>>, mut from_client: Receiver<Vec<u8>>,) -> Result<()> {
    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let idle_timeout = args.get_str("--timeout").parse::<u64>().unwrap();
    let early_data = args.get_bool("--early");
    let connection_resumption = args.get_bool("--connection-resumption");
    let poll_timeout = args.get_str("--poll-timeout").parse::<u64>().unwrap();
    
    loop { 
        // ----- QUIC Connection -----
        let mut buf = [0; 65535];
        let mut out = [0; MAX_DATAGRAM_SIZE];
        let session_file = "session_file.bin";

        // Setup the event loop.
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);

        let peer_addr = SocketAddr::V4(SocketAddrV4::new(uri.host().unwrap().parse().unwrap(), uri.port().unwrap().into()));

        // Create the UDP socket backing the QUIC connection, and register it with
        // the event loop.
        let mut socket =
            mio::net::UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        poll.registry()
            .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
            .unwrap();

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
        config.set_initial_max_stream_data_bidi_local(1_000_000_100);
        config.set_initial_max_stream_data_bidi_remote(1_000_000_100);
        config.set_initial_max_stream_data_uni(1_000_000_100);
        config.set_initial_max_streams_bidi(100_000);
        config.set_initial_max_streams_uni(100_000);
        config.set_disable_active_migration(true);
        if early_data {
            config.enable_early_data();
        }

        let mut http3_conn: Option<quiche::h3::Connection> = None;

        // Generate a random source connection ID for the connection.
        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        SystemRandom::new().fill(&mut scid[..]).unwrap();

        let scid = quiche::ConnectionId::from_ref(&scid);

        // Get local address.
        //let local_addr = socket.local_addr().unwrap();

        // Create a QUIC connection and initiate handshake.
        let mut conn =
            quiche::connect(None, &scid, peer_addr, &mut config)
                .unwrap();

            
        if let Ok(session) = std::fs::read(session_file) {
            conn.set_session(&session).ok();
        }
            

        debug!(
            "connecting to {:} from {:} with scid {}",
            peer_addr,
            socket.local_addr().unwrap(),
            hex_dump(&scid)
        );

        let (write, send_info) = conn.send(&mut out).expect("initial send failed");

        while let Err(e) = socket.send_to(&out[..write], send_info.to) {
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

        let req_start = std::time::Instant::now();

        loop {
            if !conn.is_in_early_data() {
                if poll_timeout == 0 {
                    poll.poll(&mut events, conn.timeout()).unwrap();
                }
                poll.poll(&mut events, Some(Duration::from_millis(poll_timeout))).unwrap();
            }

            // Read incoming UDP packets from the socket and feed them to quiche,
            // until there are no more packets to read.
            'read: loop {
                // If the event loop reported no events, it means that the timeout
                // has expired, so handle it without attempting to read packets. We
                // will then proceed with the send loop.
                if events.is_empty() {

                    conn.on_timeout();

                    break 'read;
                }

                let (len, from) = match socket.recv_from(&mut buf) {
                    Ok(v) => v,

                    Err(e) => {
                        // There are no more UDP packets to read, so end the read
                        // loop.
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            debug!("recv() would block");
                            break 'read;
                        } 

                       println!("recv() failed: {:?}", e);
                       break;

                        //panic!("recv() failed: {:?}", e);
                    },
                };

                info!("got {} bytes", len);

                let recv_info = quiche::RecvInfo {
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

                info!("processed {} bytes", read);
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
                    let data = match from_client.try_recv() {
                        Ok(v) => {
                            v },
                        Err(e) => {
                            if e == mpsc::error::TryRecvError::Empty {
                                //sleep(Duration::from_millis(1)).await;
                                sleep(Duration::new(0,1)).await;
                                break;
                            }

                            //println!("recv() failed: {:?}", e);
                            return Ok(());
                        },
                    };
    
                    //println!("sending HTTP request {:?}", req);
                    //println!("sending HTTP data of size {:?}", data.len());
                    //println!("{:?}", data);
                    let stream_id = h3_conn.send_request(&mut conn, &req, false).unwrap();
                    //println!("stream_id: {:?}", stream_id);
                    h3_conn.send_body(&mut conn, stream_id, &data, true).unwrap();
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
                                debug!(
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
                                "response received in {:?}, closing...",
                                req_start.elapsed()
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

                        Ok((_, quiche::h3::Event::Datagram)) => (),

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

                info!("written {}", write);
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
        //println!("[CLIENT] Received IO message of size: {:?}", msg.len());
        self.stream.write(&msg).await?;
        
        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        //println!("Received gRPC message: {:?}", msg);
        //println!("Size of msg : {:?}", msg.len());
        //println!("[CLIENT] Received gRPC message of size: {:?}", msg.len());
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

