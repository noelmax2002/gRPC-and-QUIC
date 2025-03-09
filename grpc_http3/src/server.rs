use tonic::{transport::Server, Request, Response, Status, Streaming};

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use echo::{EchoRequest, EchoResponse};
use filetransfer::file_service_server::{FileService, FileServiceServer};
use filetransfer::{FileData, FileRequest, UploadResponse};

use tokio::fs::File;
use std::collections::HashMap;
use std::net;
use tokio::io::DuplexStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task;
use quiche;
use ring::rand::*;
use log::{info,error,debug};
use std::pin::Pin;
use docopt::Docopt;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use std::{error::Error, io::ErrorKind, time::Duration};
use tokio::time::sleep;


type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
Usage: server [options]

Options:
    -s --sip ADDRESS        Server IPv4 address and port [default: 127.0.0.1:4433].
    --nocapture             Do not capture the output of the test.
    -p --proto PROTOCOL     ProtoBuf to use [default: helloworld].
    --timeout TIMEOUT       Idle timeout of the QUIC connection in milliseconds [default: 5000].
    -e --early              Enable 0-RTT.
    --poll-timeout TIMOUT   Timeout for polling the event loop in milliseconds [default: 1].
    --cert-file PATH        Path to the certificate file [default: ./cert.crt].
    --key-file PATH         Path to the private key file [default: ./cert.key].
    -t --token              Enable stateless retry token.
";

#[derive(Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> std::result::Result<Response<HelloReply>, Status> {
        println!("Got a request from {:?}", request.remote_addr());

        let reply = hello_world::HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        println!("Replying with {:?}", reply);
        Ok(Response::new(reply))
    }
}

const MAX_DATAGRAM_SIZE: usize = 1350;

struct PartialResponse {
    body: Vec<u8>,

    written: usize,
}

struct QClient {
    id: quiche::ConnectionId<'static>,

    conn: Pin<Box<quiche::Connection>>,

    http3_conn: Option<quiche::h3::Connection>,

    partial_responses: HashMap<u64, PartialResponse>,

    sending_response: bool,
}

type ClientMap = HashMap<quiche::ConnectionId<'static>, QClient>;

struct Io {
    // Towards the clients.
    clients_tx: HashMap<quiche::ConnectionId<'static>, mpsc::UnboundedSender<Vec<u8>>>,

    // To receive messages from the clients.
    rx: UnboundedReceiver<(Vec<u8>, quiche::ConnectionId<'static>)>,

    // To let clients send messages to the IO thread.
    tx: UnboundedSender<(Vec<u8>, quiche::ConnectionId<'static>)>,

    // DuplexStream with gRPC.
    to_grpc: UnboundedSender<std::result::Result<DuplexStream, String>>,
}

impl Io {
    async fn run(&mut self) -> Result<()> {

        let mut numbers_bytes_sent = 0;

        // Main task handling QUIC connections.
        let mut buf = [0; 65535];
        let mut out = [0; MAX_DATAGRAM_SIZE];

        // Setup the event loop.
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);
//127.0.0.1:4433
//192.168.1.7:8080
       
        //Read from CLI to learn the server/client address.
        let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());

        let server_addr = args.get_str("--sip").to_string();
        let idel_timeout = args.get_str("--timeout").parse::<u64>().unwrap();
        let early_data = args.get_bool("--early");
        let stateless_retry = args.get_bool("--token");
        let poll_timeout = args.get_str("--poll-timeout").parse::<u64>().unwrap();
        let cert_file = args.get_str("--cert-file").to_string();
        let key_file = args.get_str("--key-file").to_string();
       
        // Create the UDP listening socket, and register it with the event loop.
        let mut socket = mio::net::UdpSocket::bind(server_addr.parse().unwrap()).unwrap();
        poll.registry()
            .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
            .unwrap();

        // Create the configuration for the QUIC connections.
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

        if cfg!(target_os = "windows") {
            config
                .load_cert_chain_from_pem_file("src/cert.crt")
                .unwrap();
            config
                .load_priv_key_from_pem_file("src/cert.key")
                .unwrap();
        } else {
            config
                .load_cert_chain_from_pem_file(&cert_file)
                .unwrap();
            config
                .load_priv_key_from_pem_file(&key_file)
                .unwrap();
        }
        

        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();


        config.set_max_idle_timeout(idel_timeout);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(10_000_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000);
        config.set_initial_max_stream_data_bidi_remote(1_000);
        config.set_initial_max_stream_data_uni(1_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);
        if early_data {
            println!("Enabling 0-RTT");
            config.enable_early_data(); // Enable 0-RTT ? May be bad for security.
        }
       

        let h3_config = quiche::h3::Config::new().unwrap();

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();

        let mut clients = ClientMap::new();

        //let local_addr = socket.local_addr().unwrap();

        let resp = vec![
                    quiche::h3::Header::new(b":status", 200.to_string().as_bytes()),
                    quiche::h3::Header::new(b"server", b"quiche"),
                ];
        
        //let mut received_buffer = Vec::new();

        loop {
            // Find the shorter timeout from all the active connections.
            //
            // TODO: use event loop that properly supports timers
            if poll_timeout == 0 {
                let mut timeout = clients.values().filter_map(|c| c.conn.timeout()).min();
                poll.poll(&mut events, timeout).unwrap();
            } else {
                poll.poll(&mut events, Some(Duration::from_millis(poll_timeout))).unwrap();
            }

            // Read incoming UDP packets from the socket and feed them to quiche,
            // until there are no more packets to read.
            'read: loop {
                // If the event loop reported no events, it means that the timeout
                // has expired, so handle it without attempting to read packets. We
                // will then proceed with the send loop.
                if events.is_empty() {
                    debug!("timed out");

                    clients.values_mut().for_each(|c| c.conn.on_timeout());

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

                        if e.kind() == std::io::ErrorKind::ConnectionReset {
                            debug!("The connection is reset");
                            
                            break 'read;
                        }

                        panic!("recv() failed: {:?}", e);
                    },
                };

                info!("got {} bytes from {:?}", len, from);

                let pkt_buf = &mut buf[..len];

                // Parse the QUIC packet's header.
                let hdr = match quiche::Header::from_slice(
                    pkt_buf,
                    quiche::MAX_CONN_ID_LEN,
                ) {
                    Ok(v) => v,

                    Err(e) => {
                        error!("Parsing packet header failed: {:?}", e);
                        continue 'read;
                    },
                };

                info!("got packet {:?}", hdr);

                let conn_id = ring::hmac::sign(&conn_id_seed, &hdr.dcid);
                let conn_id = &conn_id.as_ref()[..quiche::MAX_CONN_ID_LEN];
                let conn_id = conn_id.to_vec().into();

                // Lookup a connection based on the packet's connection ID. If there
                // is no connection matching, create a new one.
                let client = if !clients.contains_key(&hdr.dcid) &&
                    !clients.contains_key(&conn_id)
                {
                    if hdr.ty != quiche::Type::Initial {
                        error!("Packet is not Initial");
                        continue 'read;
                    }

                    if !quiche::version_is_supported(hdr.version) {
                        info!("Doing version negotiation");

                        let len =
                            quiche::negotiate_version(&hdr.scid, &hdr.dcid, &mut out)
                                .unwrap();

                        let out = &out[..len];

                        if let Err(e) = socket.send_to(out, from) {
                            if e.kind() == std::io::ErrorKind::WouldBlock {
                                println!("send() would block");
                                break;
                            }

                            panic!("send() failed: {:?}", e);
                        }
                        continue 'read;
                    }

                    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
                    scid.copy_from_slice(&conn_id);

                    let scid = quiche::ConnectionId::from_ref(&scid);

                    let mut odcid = None;

                    if stateless_retry {
                        // Token is always present in Initial packets.
                        let token = hdr.token.as_ref().unwrap();

                        // Do stateless retry if the client didn't send a token.
                        if token.is_empty() {
                            info!("Doing stateless retry");

                            let new_token = Self::mint_token(&hdr, &from);

                            let len = quiche::retry(
                                &hdr.scid,
                                &hdr.dcid,
                                &scid,
                                &new_token,
                                hdr.version,
                                &mut out,
                            )
                            .unwrap();

                            let out = &out[..len];

                            if let Err(e) = socket.send_to(out, from) {
                                if e.kind() == std::io::ErrorKind::WouldBlock {
                                    debug!("send() would block");
                                    break;
                                }

                                panic!("send() failed: {:?}", e);
                            }
                            continue 'read;
                        }

                        let odcid = Self::validate_token(&from, token);

                        // The token was not valid, meaning the retry failed, so
                        // drop the packet.
                        if odcid.is_none() {
                            error!("Invalid address validation token");
                            continue 'read;
                        } 

                        if scid.len() != hdr.dcid.len() {
                            error!("Invalid destination connection ID");
                            continue 'read;
                        }

                        // Reuse the source connection ID we sent in the Retry packet,
                        // instead of changing it again.
                        let scid = hdr.dcid.clone();
                    } 
                    
                    let scid = quiche::ConnectionId::from_vec(scid.to_vec());
                    
                
                    println!("New Connection from {:?}", from);
                    println!("New connection: dcid={:?} scid={:?}", hdr.dcid, scid);

                    let conn = quiche::accept(
                        &scid,
                        odcid.as_ref(),
                        from,
                        &mut config,
                    )
                    .unwrap();

                    let client = QClient {
                        id: scid.clone(),
                        conn,
                        http3_conn: None,
                        partial_responses: HashMap::new(),
                        sending_response: false,
                    };

                    clients.insert(scid.clone(), client);

                    clients.get_mut(&scid).unwrap()
                } else {
                    match clients.get_mut(&hdr.dcid) {
                        Some(v) => v,

                        None => clients.get_mut(&conn_id).unwrap(),
                    }
                };

                let recv_info = quiche::RecvInfo {
                    from,
                };

                // Process potentially coalesced packets.
                let read = match client.conn.recv(pkt_buf, recv_info) {
                    Ok(v) => v,

                    Err(e) => {
                        error!("{} recv failed: {:?}", client.conn.trace_id(), e);
                        continue 'read;
                    },
                };

                debug!("{} processed {} bytes", client.conn.trace_id(), read);

                // Create a new HTTP/3 connection as soon as the QUIC connection
                // is established.
                if (client.conn.is_in_early_data() || client.conn.is_established()) &&
                    client.http3_conn.is_none() 
                {
                    println!(
                        "{} QUIC handshake completed, now trying HTTP/3",
                        client.conn.trace_id()
                    );

                    let h3_conn = match quiche::h3::Connection::with_transport(
                        &mut client.conn,
                        &h3_config,
                    ) {
                        Ok(v) => v,

                        Err(e) => {
                            error!("failed to create HTTP/3 connection: {}", e);
                            continue 'read;
                        },
                    };

                    // TODO: sanity check h3 connection before adding to map
                    client.http3_conn = Some(h3_conn);

                    info!(
                        "{} HTTP/3 connection created",
                        client.conn.trace_id()
                    );

                    // Create a new Client to handle the connection to gRPC.
                    match self.clients_tx.get(&conn_id) {
                        Some(v) => v,
                        None => {
                            // Communication with gRPC.
                            let (to_client, from_client) = tokio::io::duplex(1_000_000);
            
                            // Communication with IO.
                            let (tx, rx) = mpsc::unbounded_channel();

                            //println!("Creating new gRPC channel with id {:?}", client.id);
            
                            let mut new_client = Client {
                                stream: to_client,
                                to_io: self.tx.clone(),
                                from_io: rx,
                                id: client.id.clone(),
                            };
            
                            // Let the client run on its own.
                            tokio::spawn(async move {
                                new_client.run().await.unwrap();
                            });
                            //sleep(Duration::from_millis(10)).await;
                            sleep(Duration::new(0,1)).await;
            
                            // Notify gRPC of the new client.
                            self.to_grpc.send(Ok(from_client))?;
            
                            self.clients_tx.insert(client.id.clone(), tx);
                            self.clients_tx.get_mut(&client.id.clone()).unwrap()
                        }
                    };
            
                }

                if client.http3_conn.is_some() {
                    // Handle writable streams.
                    for stream_id in client.conn.writable() {
                        Self::handle_writable(client, stream_id);
                    }

                    // Process HTTP/3 events.
                    loop {
                        let http3_conn = client.http3_conn.as_mut().unwrap();
                        /* 
                        //try to send data from the received_buffer to client_tx
                        if !received_buffer.is_empty() {
                            let client_tx = match self.clients_tx.get(&client.id) {
                                Some(v) => v,
                                None => {
                                   //error
                                   println!("Client send data without proper http3 connection !");
                                   continue;
                                }
                            };
                            match client_tx.try_send(received_buffer.clone()) {
                                Ok(v) => v,
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    //error
                                    println!("Client send buffer is full !");
                                    continue;
                                },
                                Err(e) => {
                                    error!("gRPC channel closed: {:?}", e);
                                    break;
                                }
                            }
                            received_buffer.clear();
                        }*/

                        match http3_conn.poll(&mut client.conn) {
                            Ok((
                                _stream_id,
                                quiche::h3::Event::Headers { list: _, .. },
                            )) => {
                                // only handle data for now
                            },

                            Ok((stream_id, quiche::h3::Event::Data)) => {
                                while let Ok(read) = http3_conn.recv_body(&mut client.conn, stream_id, &mut buf) {
                                    println!(
                                        "got {} bytes of data on stream {}",
                                        read, stream_id
                                    );

                                    //send data to gRPC
                                    let client_tx = match self.clients_tx.get(&client.id) {
                                        Some(v) => v,
                                        None => {
                                           //error
                                           //println!("Client send data without proper http3 connection !");
                                           error!("Client send data without proper http3 connection !");
                                           continue;
                                        }
                                    };
                                    match client_tx.send(buf[..read].to_vec()) {
                                        Ok(v) => v,
                                        /* 
                                        Err(mpsc::error::TrySendError::Full(_)) => {
                                            //error
                                            println!("Client send buffer is full !");
                                            //received_buffer.extend_from_slice(&buf[..read]);
                                            sleep(Duration::new(0,1)).await;

                                            continue;
                                        },*/
                                        Err(e) => {
                                            println!("gRPC channel closed: {:?}", e);
                                            break;
                                        }
                                    }
                                    numbers_bytes_sent += read;
                                    println!("Total bytes sent: {:?}", numbers_bytes_sent);

                                   //sleep(Duration::from_millis(1)).await;
                                    sleep(Duration::new(0,1)).await;

                                }
                                //client.conn.stream_shutdown(stream_id, quiche::Shutdown::Read, 0).unwrap();
                            },

                            Ok((_stream_id, quiche::h3::Event::Finished)) => (),

                            Ok((_stream_id, quiche::h3::Event::Reset { .. })) => (),

                            Ok((_goaway_id, quiche::h3::Event::GoAway)) => (),

                            Ok((_, quiche::h3::Event::Datagram)) => (),

                            Err(quiche::h3::Error::Done) => {
                                break;
                            },

                            Err(e) => {
                                error!(
                                    "{} HTTP/3 error {:?}",
                                    client.conn.trace_id(),
                                    e
                                );

                                break;
                            },
                        }
                    }
                }
            }

            // Send gRPC message 
            loop {
                let (data, id) = match self.rx.try_recv(){
                    Ok(v) => v,
                    Err(mpsc::error::TryRecvError::Empty) => {
                        debug!("No more data to send");
                        break;
                    },
                    Err(e) => {
                        debug!("Channel closed: {:?}", e);
                        break;
                    }
                    
                };

                //println!("sending HTTP response {:?}", resp);
                //println!("{:?}", data);
                println!("sending HTTP response of size {:?}", data.len());
                let client = match clients.get_mut(&id) {
                    Some(v) => v,
                    None => {
                       //error
                       println!("Client not found in the client array !");      
                       continue;
                    }
                };
                let stream_id = 0;
                let h3_conn = client.http3_conn.as_mut().unwrap();
                let conn = &mut client.conn;

                if !client.sending_response {
                    match h3_conn.send_response(conn, stream_id, &resp, false) {
                        Ok(v) => v,
                
                        Err(quiche::h3::Error::StreamBlocked) => {
                            println!("Stream blocked");
                            let response = PartialResponse {
                                body: data,
                                written: 0,
                            };
                
                            client.partial_responses.insert(stream_id, response);
                            continue;
                        },
                
                        Err(e) => {
                            error!("{} stream send failed {:?}", conn.trace_id(), e);
                            continue;
                        },
                    }
                    client.sending_response = true;
                }
            
                // If last bytes of data are equal to [218,143,129,7] then its the end of the gRPC response
                // This an exprimental conclusion, it may not work in all cases.
                let mut stop = false;
                let end = data.len();
                if end >= 4 && data[end-4] == 218 && data[end-3] == 143 && data[end-2] == 129 && data[end-1] == 7{
                    stop = false; //must be set to true but leads to problem for now
                }

                let written = match h3_conn.send_body(conn, stream_id, &data, stop) {
                    Ok(v) => v,
            
                    Err(quiche::h3::Error::Done) => 0,
            
                    Err(e) => {
                        error!("{} stream send failed {:?}", conn.trace_id(), e);
                        continue;
                    },
                };

                if written < end{
                    let response = PartialResponse {
                        body: data,
                        written,
                    };

                    client.partial_responses.insert(stream_id, response);
                } else {
                    if written >= 4 && data[written-4] == 218 && data[written-3] == 143 && data[written-2] == 129 && data[written-1] == 7{
                        println!("-End of the gRPC response-");
                        client.sending_response = false;
                    }
                }
               
            }
            


            // Generate outgoing QUIC packets for all active connections and send
            // them on the UDP socket, until quiche reports that there are no more
            // packets to be sent.
            for client in clients.values_mut() {
                loop {
                    let (write, send_info) = match client.conn.send(&mut out) {
                        Ok(v) => v,

                        Err(quiche::Error::Done) => {
                            debug!("{} done writing", client.conn.trace_id());
                            break;
                        },

                        Err(e) => {
                            error!("{} send failed: {:?}", client.conn.trace_id(), e);

                            client.conn.close(false, 0x1, b"fail").ok();
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

                    debug!("{} written {} bytes", client.conn.trace_id(), write);
                }
            }

            // Garbage collect closed connections.
            clients.retain(|_, ref mut c| {
                info!("Collecting garbage");

                if c.conn.is_closed() {
                    println!(
                        "{} connection collected {:?}",
                        c.conn.trace_id(),
                        c.conn.stats()
                    );
                }

                !c.conn.is_closed()
            });

            self.clients_tx.retain(|k, _| {
                clients.contains_key(k)
            });

        }
    }

    /// Generate a stateless retry token.
    ///
    /// The token includes the static string `"quiche"` followed by the IP address
    /// of the client and by the original destination connection ID generated by the
    /// client.
    ///
    /// Note that this function is only an example and doesn't do any cryptographic
    /// authenticate of the token. *It should not be used in production system*.
    fn mint_token(hdr: &quiche::Header, src: &net::SocketAddr) -> Vec<u8> {
        let mut token = Vec::new();

        token.extend_from_slice(b"quiche");

        let addr = match src.ip() {
            std::net::IpAddr::V4(a) => a.octets().to_vec(),
            std::net::IpAddr::V6(a) => a.octets().to_vec(),
        };

        token.extend_from_slice(&addr);
        token.extend_from_slice(&hdr.dcid);

        token
    }

    /// Validates a stateless retry token.
    ///
    /// This checks that the ticket includes the `"quiche"` static string, and that
    /// the client IP address matches the address stored in the ticket.
    ///
    /// Note that this function is only an example and doesn't do any cryptographic
    /// authenticate of the token. *It should not be used in production system*.
    fn validate_token<'a>(
        src: &net::SocketAddr, token: &'a [u8],
    ) -> Option<quiche::ConnectionId<'a>> {
        if token.len() < 6 {
            return None;
        }

        if &token[..6] != b"quiche" {
            return None;
        }

        let token = &token[6..];

        let addr = match src.ip() {
            std::net::IpAddr::V4(a) => a.octets().to_vec(),
            std::net::IpAddr::V6(a) => a.octets().to_vec(),
        };

        if token.len() < addr.len() || &token[..addr.len()] != addr.as_slice() {
            return None;
        }

        Some(quiche::ConnectionId::from_ref(&token[addr.len()..]))
    }

    /// Handles newly writable streams.
    fn handle_writable(client: &mut QClient, stream_id: u64) {
        let conn = &mut client.conn;
        let http3_conn = &mut client.http3_conn.as_mut().unwrap();

        debug!("{} stream {} is writable", conn.trace_id(), stream_id);

        if !client.partial_responses.contains_key(&stream_id) {
            return;
        }

        let resp = client.partial_responses.get_mut(&stream_id).unwrap();

        let headers = vec![
            quiche::h3::Header::new(b":status", 200.to_string().as_bytes()),
            quiche::h3::Header::new(b"server", b"quiche"),
        ];

        if !client.sending_response {
            match http3_conn.send_response(conn, stream_id, &headers, false) {
                Ok(_) => (),

                Err(quiche::h3::Error::StreamBlocked) => {
                    return;
                },

                Err(e) => {
                    error!("{} stream send failed {:?}", conn.trace_id(), e);
                    return;
                },
            }

            client.sending_response = true;
        }

        let body = &resp.body[resp.written..];

        let mut stop = false;
        let end = body.len();
        if end >= 4 && body[end-4] == 218 && body[end-3] == 143 && body[end-2] == 129 && body[end-1] == 7{
            stop = false; //must be set true but lead to problems for now
        }
        let written = match http3_conn.send_body(conn, stream_id, body, stop) {
            Ok(v) => v,

            Err(quiche::h3::Error::Done) => 0,

            Err(e) => {
                client.partial_responses.remove(&stream_id);

                error!("{} stream send failed {:?}", conn.trace_id(), e);
                return;
            },
        };

        resp.written += written;

        if resp.written == resp.body.len() {
            if stop {
                println!("-End of the gRPC response-");
                client.sending_response = false;
            }

            client.partial_responses.remove(&stream_id);
        } else {
            println!("{} stream {} written {} bytes", conn.trace_id(), stream_id, written);
        }
    }
}

struct Client {
    stream: DuplexStream,
    to_io: UnboundedSender<(Vec<u8>, quiche::ConnectionId<'static>)>,
    from_io: UnboundedReceiver<Vec<u8>>,
    id: quiche::ConnectionId<'static>,
}

impl Client {
    async fn run(&mut self) -> Result<()> {
        let mut buf = [0u8; 1500];
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => {
                    if len == 0 {
                        println!("Channel to gRPC closed");
                        return Ok(());
                    }

                    self.handle_grpc_msg(&buf[..len]).await?
                },
            }

            if self.from_io.is_closed() {
                println!("Channel from IO closed");
                return Ok(());
            }

        }
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        //println!("[SERVER] Client got a message from IO: {:?}", msg);
        println!("[SERVER] Received IO message of size: {:?}", msg.len());
        //sleep(Duration::from_millis(1)).await;
         
        let vec_to_string = String::from_utf8_lossy(&msg);
        println!("{:?}", msg);
        println!("{}", vec_to_string); 
         
        self.stream.write(&msg).await?;
        
        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        println!("[SERVER] gRPC message of size: {:?}", msg.len());
        //println!("Received gRPC message: {:?}", msg);
        
        let vec_to_string = String::from_utf8_lossy(msg.clone());
        println!("{:?}", msg);
        println!("{}", vec_to_string); 
         
        self.to_io.send((msg.to_vec(), self.id.clone()));
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    launch_server().await?;

    Ok(())
}

pub async fn launch_server() -> Result<()> {
    let (to_tonic, from_tonic) =
        mpsc::unbounded_channel::<std::result::Result<DuplexStream, String>>();
    
    // To let clients communicate with the main thread.
    let (tx, rx) = mpsc::unbounded_channel();

    let mut io = Io {
        clients_tx: HashMap::new(),
        tx,
        rx,
        to_grpc: to_tonic,
    };

    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let proto = args.get_str("--proto").to_string();
    
    // Create tonic server builder.
    if proto == "helloworld" {
        let greeter = MyGreeter::default(); //Define the gRPC service.
        task::spawn(async move {
            Server::builder()
                .add_service(GreeterServer::new(greeter))
                .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    from_tonic,
                ))
                .await
                .unwrap();
        });
    } else if proto == "echo" {
        let echo = EchoServer {};
        task::spawn(async move {
            Server::builder()
                .add_service(echo::echo_server::EchoServer::new(echo))
                .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    from_tonic,
                ))
                .await
                .unwrap();
        });

    } else if proto == "filetransfer" {
        let file_service = MyFileService::default();
        task::spawn(async move {
            Server::builder()
                .add_service(FileServiceServer::new(file_service))
                .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(
                    from_tonic,
                ))
                .await
                .unwrap();
        });
    } else {
        panic!("Unknown protocol: {:?}", proto);
    }

    

    io.run().await?;

    Ok(())
}

//Code from tonic/examples/src/streaming/server.rs
type EchoResult<T> = std::result::Result<Response<T>, Status>;
type ResponseStream = Pin<Box<dyn Stream<Item = std::result::Result<EchoResponse, Status>> + Send>>;

fn match_for_io_error(err_status: &Status) -> Option<&std::io::Error> {
    let mut err: &(dyn Error + 'static) = err_status;

    loop {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            return Some(io_err);
        }
        err = match err.source() {
            Some(err) => err,
            None => return None,
        };
    }
}

#[derive(Debug)]
pub struct EchoServer {}

#[tonic::async_trait]
impl echo::echo_server::Echo for EchoServer {
    async fn unary_echo(&self, _: Request<EchoRequest>) -> EchoResult<EchoResponse> {
        Err(Status::unimplemented("not implemented"))
    }

    type ServerStreamingEchoStream = ResponseStream;

    async fn server_streaming_echo(
        &self,
        req: Request<EchoRequest>,
    ) -> EchoResult<Self::ServerStreamingEchoStream> {
        println!("EchoServer::server_streaming_echo");
        println!("\tclient connected from: {:?}", req.remote_addr());

        // creating infinite stream with requested message
        let repeat = std::iter::repeat(EchoResponse {
            message: req.into_inner().message,
        });
        let mut stream = Box::pin(tokio_stream::iter(repeat).throttle(Duration::from_millis(200)));

        // spawn and channel are required if you want handle "disconnect" functionality
        // the `out_stream` will not be polled after client disconnect
        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            while let Some(item) = stream.next().await {
                match tx.send(std::result::Result::<_, Status>::Ok(item)).await {
                    Ok(_) => {
                        // item (server response) was queued to be send to client
                    }
                    Err(_item) => {
                        // output_stream was build from rx and both are dropped
                        break;
                    }
                }
            }
            println!("\tclient disconnected");
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::ServerStreamingEchoStream
        ))
    }

    async fn client_streaming_echo(
        &self,
        _: Request<Streaming<EchoRequest>>,
    ) -> EchoResult<EchoResponse> {
        Err(Status::unimplemented("not implemented"))
    }

    type BidirectionalStreamingEchoStream = ResponseStream;

    async fn bidirectional_streaming_echo(
        &self,
        req: Request<Streaming<EchoRequest>>,
    ) -> EchoResult<Self::BidirectionalStreamingEchoStream> {
        println!("EchoServer::bidirectional_streaming_echo");

        let mut in_stream = req.into_inner();
        let (tx, rx) = mpsc::channel(128);

        // this spawn here is required if you want to handle connection error.
        // If we just map `in_stream` and write it back as `out_stream` the `out_stream`
        // will be dropped when connection error occurs and error will never be propagated
        // to mapped version of `in_stream`.
        tokio::spawn(async move {
            while let Some(result) = in_stream.next().await {
                match result {
                    Ok(v) => tx
                        .send(Ok(EchoResponse { message: v.message }))
                        .await
                        .expect("working rx"),
                    Err(err) => {
                        if let Some(io_err) = match_for_io_error(&err) {
                            if io_err.kind() == ErrorKind::BrokenPipe {
                                // here you can handle special case when client
                                // disconnected in unexpected way
                                eprintln!("\tclient disconnected: broken pipe");
                                break;
                            }
                        }

                        match tx.send(Err(err)).await {
                            Ok(_) => (),
                            Err(_err) => break, // response was dropped
                        }
                    }
                }
            }
            println!("\tstream ended");
        });

        // echo just write the same data that was received
        let out_stream = ReceiverStream::new(rx);

        Ok(Response::new(
            Box::pin(out_stream) as Self::BidirectionalStreamingEchoStream
        ))
    }
}

#[derive(Default)]
pub struct MyFileService;

#[tonic::async_trait]
impl FileService for MyFileService {
    /// Handles file upload as a single `bytes` field
    async fn upload_file(
        &self,
        request: Request<FileData>,
    ) -> std::result::Result<Response<UploadResponse>, Status> {
        let file = request.into_inner();
        let mut new_file = File::create(file.filename).await.map_err(|e| Status::internal(e.to_string()))?;
        new_file.write_all(&file.data).await.map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(UploadResponse { message: "File uploaded successfully".into() }))
    }

    /// Handles file download as a single `bytes` field
    async fn download_file(
        &self,
        request: Request<FileRequest>,
    ) -> std::result::Result<Response<FileData>, Status> {
        let filename = request.into_inner().filename;
        let mut file = File::open(&filename).await.map_err(|e| Status::not_found(e.to_string()))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await.map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(FileData { filename, data: buffer }))
    }
}

/// Validates a stateless retry token.
///
/// This checks that the ticket includes the `"quiche"` static string, and that
/// the client IP address matches the address stored in the ticket.
///
/// Note that this function is only an example and doesn't do any cryptographic
/// authenticate of the token. *It should not be used in production system*.
fn validate_token<'a>(
    src: &net::SocketAddr, token: &'a [u8],
) -> Option<quiche::ConnectionId<'a>> {
    if token.len() < 6 {
        return None;
    }

    if &token[..6] != b"quiche" {
        return None;
    }

    let token = &token[6..];

    let addr = match src.ip() {
        std::net::IpAddr::V4(a) => a.octets().to_vec(),
        std::net::IpAddr::V6(a) => a.octets().to_vec(),
    };

    if token.len() < addr.len() || &token[..addr.len()] != addr.as_slice() {
        return None;
    }

    Some(quiche::ConnectionId::from_ref(&token[addr.len()..]))
}

