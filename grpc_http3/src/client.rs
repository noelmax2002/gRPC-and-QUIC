use std::net::{SocketAddr, SocketAddrV4};

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::{Endpoint,Uri};
use tower::service_fn;
use hyper_util::rt::tokio::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc;
use tokio::task;
use log::{info, error};
use tokio::time::Duration;
use tokio::time::sleep;

use quiche::h3::NameValue;

use ring::rand::*;

const MAX_DATAGRAM_SIZE: usize = 1350;

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
                let (to_client, from_io) = mpsc::channel(1000);
                let (to_io, mut from_client) = mpsc::channel(1000);

                let mut new_client = Client {
                    stream: client,
                    to_io: to_io,
                    from_io: from_io,
                };
            
                println!("---------Client created");
                // Let the client run on its own.
                tokio::spawn(async move {
                    println!("---------Client spawned");
                    new_client.run().await.unwrap();
                });



                run_client(uri, to_client, from_client).await.unwrap();
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

async fn run_client(uri: Uri, to_client: Sender<Vec<u8>>, mut from_client: Receiver<Vec<u8>>,) -> Result<()> {
  
    // ----- QUIC Connection -----
    let mut buf = [0; 65535];
    let mut out = [0; MAX_DATAGRAM_SIZE];

    // Setup the event loop.
    let mut poll = mio::Poll::new().unwrap();
    let mut events = mio::Events::with_capacity(1024);

    let peer_addr = SocketAddr::V4(SocketAddrV4::new(uri.host().unwrap().parse().unwrap(), uri.port().unwrap().into()));

    // Create the UDP socket backing the QUIC connection, and register it with
    // the event loop.
    let mut socket =
        mio::net::UdpSocket::bind("0.0.0.0:0".parse().unwrap()).unwrap();
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

    config.set_max_idle_timeout(500000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000_100);
    config.set_initial_max_stream_data_bidi_remote(1_000_000_100);
    config.set_initial_max_stream_data_uni(1_000_000_100);
    config.set_initial_max_streams_bidi(100_000);
    config.set_initial_max_streams_uni(100_000);
    config.set_disable_active_migration(true);

    let mut http3_conn = None;

    // Generate a random source connection ID for the connection.
    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    SystemRandom::new().fill(&mut scid[..]).unwrap();

    let scid = quiche::ConnectionId::from_ref(&scid);

    // Get local address.
    let local_addr = socket.local_addr().unwrap();

    // Create a QUIC connection and initiate handshake.
    let mut conn =
        quiche::connect(None, &scid, local_addr, peer_addr, &mut config)
            .unwrap();

    println!(
        "connecting to {:} from {:} with scid {}",
        peer_addr,
        socket.local_addr().unwrap(),
        hex_dump(&scid)
    );

    let (write, send_info) = conn.send(&mut out).expect("initial send failed");

    while let Err(e) = socket.send_to(&out[..write], send_info.to) {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            println!("send() would block");
            continue;
        }

        panic!("send() failed: {:?}", e);
    }

    println!("written {}", write);

    let h3_config = quiche::h3::Config::new().unwrap();

    // Prepare request. (dummy request for now)
    let req = vec![
        quiche::h3::Header::new(b":method", b"PUSH"),
    ];

    let req_start = std::time::Instant::now();

    let mut req_sent = false;

    loop {
        poll.poll(&mut events, conn.timeout()).unwrap();

        // Read incoming UDP packets from the socket and feed them to quiche,
        // until there are no more packets to read.
        'read: loop {
            // If the event loop reported no events, it means that the timeout
            // has expired, so handle it without attempting to read packets. We
            // will then proceed with the send loop.
            if events.is_empty() {
                println!("timed out");

                conn.on_timeout();

                break 'read;
            }

            let (len, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,

                Err(e) => {
                    // There are no more UDP packets to read, so end the read
                    // loop.
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        println!("recv() would block");
                        break 'read;
                    }

                    panic!("recv() failed: {:?}", e);
                },
            };

            println!("got {} bytes", len);

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

            println!("processed {} bytes", read);
        }

        println!("done reading");

        if conn.is_closed() {
            info!("connection closed, {:?}", conn.stats());
            break Ok(());
        }

        // Create a new HTTP/3 connection once the QUIC connection is established.
        if conn.is_established() && http3_conn.is_none() {
            http3_conn = Some(
                quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                .expect("Unable to create HTTP/3 connection, check the server's uni stream limit and window size"),
            );
        }

        // Send HTTP requests once the QUIC connection is established, and until
        // all requests have been sent.
        if let Some(h3_conn) = &mut http3_conn {
            println!("Waiting for gRPC data");
            loop{
                let data = match from_client.try_recv() {
                    Ok(v) => v,
                    Err(e) => {
                        if e == mpsc::error::TryRecvError::Empty {
                            println!("no data to send");
                            sleep(Duration::from_millis(10)).await;
                            break;
                        }

                        panic!("recv() failed: {:?}", e);
                    },
                };

                //divide data if greater than MAX_DATAGRAM_SIZE
                println!("Data size: {:?}", data.len());
                
                /* 
                let mut data = data;
                let accepted_size = MAX_DATAGRAM_SIZE/8 - 167;
                println!("Accepted size: {:?}", accepted_size);
                while data.len() > accepted_size {
                    let (first, second) = data.split_at(accepted_size);
                    println!("sending HTTP request {:?}", req);
                    println!("{:?}", first);
                    let stream_id = h3_conn.send_request(&mut conn, &req, false).unwrap();
                    h3_conn.send_body(&mut conn, stream_id, &first, false).unwrap();
                    data = second.to_vec();
                } */
                
                
                println!("sending HTTP request {:?}", req);
                println!("{:?}", data);
                let stream_id = h3_conn.send_request(&mut conn, &req, false).unwrap();
                h3_conn.send_body(&mut conn, stream_id, &data, true).unwrap();
            }
                

            /* 
            if !req_sent {
                info!("sending HTTP request {:?}", req);

                h3_conn.send_request(&mut conn, &req, true).unwrap();

                req_sent = true;
            }
            */
        }
        
        if let Some(http3_conn) = &mut http3_conn {
            // Process HTTP/3 events.
            loop {
                match http3_conn.poll(&mut conn) {
                    Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                        println!(
                            "got response headers {:?} on stream id {}",
                            hdrs_to_strings(&list),
                            stream_id
                        );
                    },

                    Ok((stream_id, quiche::h3::Event::Data)) => {
                        while let Ok(read) =
                            http3_conn.recv_body(&mut conn, stream_id, &mut buf)
                        {
                            println!(
                                "got {} bytes of response data on stream {}",
                                read, stream_id
                            );

                            println!("{:?}", &buf[..read]);

                            to_client.send(buf[..read].to_vec()).await?;
                            sleep(Duration::from_millis(10)).await;
                        }
                    },

                    Ok((_stream_id, quiche::h3::Event::Finished)) => {
                        println!(
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

                    Ok((_, quiche::h3::Event::PriorityUpdate)) => unreachable!(),

                    Ok((goaway_id, quiche::h3::Event::GoAway)) => {
                        println!("GOAWAY id={}", goaway_id);
                    },

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
                    println!("done writing");
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
                    println!("send() would block");
                    break;
                }

                panic!("send() failed: {:?}", e);
            }

            println!("written {}", write);
        }

        if conn.is_closed() {
            println!("connection closed, {:?}", conn.stats());
            break Ok(());
        }
    }
}

struct Client {
    stream: DuplexStream,
    to_io: Sender<Vec<u8>>,
    from_io: Receiver<Vec<u8>>,
}

impl Client {
    // Handle the communication between Quic (synchronous) thread and gRPC (asynchronous) thread.
    async fn run(&mut self) -> Result<()> {
        println!("---------Client running");
        let mut buf = [0u8; 1500];
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => self.handle_grpc_msg(&buf[..len]).await?,
            }
        }
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        println!("Received IO message: {:?}", msg);
        self.stream.write(&msg).await?;
        
        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        println!("Received gRPC message: {:?}", msg);
        println!("Size of msg : {:?}", msg.len());
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