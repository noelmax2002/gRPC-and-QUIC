use tonic::{transport::Server, Request, Response, Status};

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

use std::collections::HashMap;
use std::net;
use tokio::io::DuplexStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task;
use quiche;
use ring::rand::*;
use log::{info,error,debug};
//use tokio::time::{sleep,Duration};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

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
    headers: Option<Vec<quiche::h3::Header>>,

    body: Vec<u8>,

    written: usize,
}

struct QClient {
    id: quiche::ConnectionId<'static>,

    conn: quiche::Connection,

    http3_conn: Option<quiche::h3::Connection>,

    partial_responses: HashMap<u64, PartialResponse>,
}

type ClientMap = HashMap<quiche::ConnectionId<'static>, QClient>;

struct Io {
    // Towards the clients.
    clients_tx: HashMap<quiche::ConnectionId<'static>, mpsc::Sender<Vec<u8>>>,

    // To receive messages from the clients.
    rx: Receiver<(Vec<u8>, quiche::ConnectionId<'static>)>,

    // To let clients send messages to the IO thread.
    tx: Sender<(Vec<u8>, quiche::ConnectionId<'static>)>,

    // DuplexStream with gRPC.
    to_grpc: UnboundedSender<std::result::Result<DuplexStream, String>>,
}

impl Io {
    async fn run(&mut self) -> Result<()> {
        // Main task handling QUIC connections.
        let mut buf = [0; 65535];
        let mut out = [0; MAX_DATAGRAM_SIZE];

        // Setup the event loop.
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);

        // Create the UDP listening socket, and register it with the event loop.
        let mut socket = mio::net::UdpSocket::bind("127.0.0.1:4433".parse().unwrap()).unwrap();
        poll.registry()
            .register(&mut socket, mio::Token(0), mio::Interest::READABLE)
            .unwrap();

        // Create the configuration for the QUIC connections.
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

        config
            .load_cert_chain_from_pem_file("src/cert.crt")
            .unwrap();
        config
            .load_priv_key_from_pem_file("src/cert.key")
            .unwrap();

        config
            .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
            .unwrap();

        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config.set_disable_active_migration(true);
        config.enable_early_data(); // Enable 0-RTT ? May be bad for security.

        let h3_config = quiche::h3::Config::new().unwrap();

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();

        let mut clients = ClientMap::new();

        let local_addr = socket.local_addr().unwrap();

        let resp = vec![
                    quiche::h3::Header::new(b":status", 200.to_string().as_bytes()),
                    quiche::h3::Header::new(b"server", b"quiche"),
                ];

        loop {
            // Find the shorter timeout from all the active connections.
            //
            // TODO: use event loop that properly supports timers
            let timeout = clients.values().filter_map(|c| c.conn.timeout()).min();

            poll.poll(&mut events, timeout).unwrap();

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

                info!("got {} bytes", len);

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
                                debug!("send() would block");
                                break;
                            }

                            panic!("send() failed: {:?}", e);
                        }
                        continue 'read;
                    }

                    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
                    scid.copy_from_slice(&conn_id);

                    let scid = quiche::ConnectionId::from_ref(&scid);

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

                    println!("New connection: dcid={:?} scid={:?}", hdr.dcid, scid);

                    let conn = quiche::accept(
                        &scid,
                        odcid.as_ref(),
                        local_addr,
                        from,
                        &mut config,
                    )
                    .unwrap();

                    let client = QClient {
                        id: scid.clone(),
                        conn,
                        http3_conn: None,
                        partial_responses: HashMap::new(),
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
                    to: socket.local_addr().unwrap(),
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

                info!("{} processed {} bytes", client.conn.trace_id(), read);

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
                            let (to_client, from_client) = tokio::io::duplex(1000);
            
                            // Communication with IO.
                            let (tx, rx) = mpsc::channel(1000);

                            println!("Creating new client with id {:?}", client.id);
            
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
                                           println!("Client send data without proper http3 connection !");
                                           error!("Client send data without proper http3 connection !");
                                           continue;
                                        }
                                    };
                                    client_tx.send(buf[..read].to_vec()).await?;
                                    //sleep(Duration::from_millis(1)).await;
                                }
                                //client.conn.stream_shutdown(stream_id, quiche::Shutdown::Read, 0).unwrap();
                            },

                            Ok((_stream_id, quiche::h3::Event::Finished)) => (),

                            Ok((_stream_id, quiche::h3::Event::Reset { .. })) => (),

                            Ok((
                                _prioritized_element_id,
                                quiche::h3::Event::PriorityUpdate,
                            )) => (),

                            Ok((_goaway_id, quiche::h3::Event::GoAway)) => (),

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

                println!("sending HTTP response {:?}", resp);
                println!("{:?}", data);
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
                match h3_conn.send_response(conn, stream_id, &resp, false) {
                    Ok(v) => v,
            
                    Err(quiche::h3::Error::StreamBlocked) => {
                        let response = PartialResponse {
                            headers: Some(resp.clone()),
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
            
                match h3_conn.send_body(conn, stream_id, &data, false) {
                    Ok(v) => v,
            
                    Err(quiche::h3::Error::Done) => 0,
            
                    Err(e) => {
                        error!("{} stream send failed {:?}", conn.trace_id(), e);
                        continue;
                    },
                };
               
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

                    info!("{} written {} bytes", client.conn.trace_id(), write);
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

        if let Some(ref headers) = resp.headers {
            debug!("{} stream send response {:?}", conn.trace_id(), headers);
            match http3_conn.send_response(conn, stream_id, headers, false) {
                Ok(_) => (),

                Err(quiche::h3::Error::StreamBlocked) => {
                    return;
                },

                Err(e) => {
                    error!("{} stream send failed {:?}", conn.trace_id(), e);
                    return;
                },
            }
        }

        resp.headers = None;

        let body = &resp.body[resp.written..];

        let written = match http3_conn.send_body(conn, stream_id, body, true) {
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
            client.partial_responses.remove(&stream_id);
        }
    }
}

struct Client {
    stream: DuplexStream,
    to_io: Sender<(Vec<u8>, quiche::ConnectionId<'static>)>,
    from_io: Receiver<Vec<u8>>,
    id: quiche::ConnectionId<'static>,
}

impl Client {
    async fn run(&mut self) -> Result<()> {
        let mut buf = [0u8; 1500];
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => self.handle_grpc_msg(&buf[..len]).await?,
            }
        }
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        println!("Client got a message from IO: {:?}", msg);
        self.stream.write(&msg).await?;
        
        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
        self.to_io.send((msg.to_vec(), self.id.clone())).await?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (to_tonic, from_tonic) =
        mpsc::unbounded_channel::<std::result::Result<DuplexStream, String>>();
    let greeter = MyGreeter::default(); //Define the gRPC service.

    // To let clients communicate with the main thread.
    let (tx, rx) = mpsc::channel(1000);

    /* 
    let addr: SocketAddr = "127.0.0.1:4433".parse().unwrap();
    println!("GreeterServer listening on {:?}", addr);

    let socket = UdpSocket::bind(addr).await?;
    */

    let mut io = Io {
        clients_tx: HashMap::new(),
        tx,
        rx,
        to_grpc: to_tonic,
    };

    // Create tonic server builder.
    task::spawn(async move {
        Server::builder()
            .add_service(GreeterServer::new(greeter))
            .serve_with_incoming(tokio_stream::wrappers::UnboundedReceiverStream::new(
                from_tonic,
            ))
            .await
            .unwrap();
    });

    io.run().await?;

    Ok(())
}
