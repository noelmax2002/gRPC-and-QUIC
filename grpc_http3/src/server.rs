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
use quiche::ConnectionId;
use ring::rand::*;
use log::{info,error,debug, trace};
use std::pin::Pin;
use docopt::Docopt;
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use std::{error::Error, io::ErrorKind, time::Duration};
use tokio::time::sleep;
use tokio::time::Instant;
use std::sync::Arc;

use ring::rand::*;
use rand::seq::SliceRandom;
use rand::thread_rng;


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
    -s --sip ADDRESS            Server IPv4 address and port [default: 127.0.0.1:4433].
    --nocapture                 Do not capture the output of the test.
    -p --proto PROTOCOL         ProtoBuf to use [default: helloworld].
    --timeout TIMEOUT           Idle timeout of the QUIC connection in milliseconds [default: 5000].
    -e --early                  Enable 0-RTT.
    --poll-timeout TIMOUT       Timeout for polling the event loop in milliseconds [default: 1].
    --cert-file PATH            Path to the certificate file [default: cert.crt].
    --key-file PATH             Path to the private key file [default: cert.key].
    --multipath                 Enable multipath.
    --scheduler SCHEDULER       Choose the scheduler to use [default: lrtt].
    --rcvd-threshold TIMEOUT    Timeout for the rcvd threshold in milliseconds [default: 25].
    --token                     Enable stateless retry token.
    --max-bidi-remote SIZE      Max stream data for remote [default: 10000].
    --ack-eliciting-timer TIMEOUT   Timeout for the ack elicting timer in milliseconds [default: 25].
    --no-pacing                 Disable pacing.
    --no-cc                     Disable congestion control.
    --duplicate                 Enable duplicate on path.
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


const MAX_DATAGRAM_SIZE: usize = 1500;
const MAX_BRIDGING_BUFFER_SIZE: usize = 100_000;
const MAX_BUF_SIZE: usize = 65507;
pub type ClientId = u64;

struct PartialResponse {
    body: Vec<u8>,

    written: usize,
}

struct QClient {
    id: quiche::ConnectionId<'static>,

    conn: Box<quiche::Connection>,

    http3_conn: Option<quiche::h3::Connection>,

    pub client_id: ClientId,

    partial_responses: HashMap<u64, PartialResponse>,

    sending_response: bool,

    pub max_datagram_size: usize,

    pub loss_rate: f64,

    pub max_send_burst: usize,

    pub last_addr_recv: std::net::SocketAddr,

    pub last_primary_path: std::net::SocketAddr,

    pub rtt_map: HashMap<std::net::SocketAddr, Duration>,

    pub cut_map: HashMap<std::net::SocketAddr, bool>,
}

pub type ClientIdMap = HashMap<ConnectionId<'static>, ClientId>;
type ClientMap = HashMap<ClientId, QClient>;

struct Io {
    // Towards the clients.
    clients_tx: HashMap<ClientId, mpsc::UnboundedSender<Vec<u8>>>,

    // To receive messages from the clients.
    rx: UnboundedReceiver<(Vec<u8>, ClientId)>,

    // To let clients send messages to the IO thread.
    tx: UnboundedSender<(Vec<u8>, ClientId)>,

    // DuplexStream with gRPC.
    to_grpc: UnboundedSender<std::result::Result<DuplexStream, String>>,
}

impl Io {
    async fn run(&mut self) -> Result<()> {
        // Main task handling QUIC connections.
        let mut buf = [0; MAX_BUF_SIZE];
        let mut out = [0; MAX_BUF_SIZE];

        // Setup the event loop.
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);

        let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());

        let server_addr = args.get_str("--sip").to_string();
        let idle_timeout = args.get_str("--timeout").parse::<u64>().unwrap();
        let early_data = args.get_bool("--early");
        let stateless_retry = args.get_bool("--token");
        let poll_timeout = args.get_str("--poll-timeout").parse::<u64>().unwrap();
        let cert_file = args.get_str("--cert-file").to_string();
        let key_file = args.get_str("--key-file").to_string();
        let multipath = args.get_bool("--multipath");
        let scheduler = args.get_str("--scheduler");
        let rcvd_threshold = args.get_str("--rcvd-threshold").parse::<u64>().unwrap();
        let duplicate = args.get_bool("--duplicate");
        let max_bidi_remote = args.get_str("--max-bidi-remote").parse::<u64>().unwrap();
        let ack_eliciting_timer = args.get_str("--ack-eliciting-timer").parse::<u64>().unwrap();
        let no_pacing = args.get_bool("--no-pacing");
        let no_cc = args.get_bool("--no-cc");

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


        config.set_max_idle_timeout(idle_timeout);
        config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
        config.set_initial_max_data(10_000_000_000);
        config.set_initial_max_stream_data_bidi_local(10_000); 
        config.set_initial_max_stream_data_bidi_remote(max_bidi_remote); //this parameter improves the performance
        config.set_initial_max_stream_data_uni(1000);
        config.set_initial_max_streams_bidi(1000);
        config.set_initial_max_streams_uni(1000);
        config.set_disable_active_migration(false);
        config.enable_pacing(!no_pacing);

        if duplicate {
            config.set_cc_algorithm(quiche::CongestionControlAlgorithm::DISABLED);
        } else if multipath {
            config.set_max_ack_delay(rcvd_threshold);
        }
        if early_data {
            config.enable_early_data(); // Enable 0-RTT ? May be bad for security.
        }

        if multipath {
            config.set_initial_max_path_id(100);
        } else {
            config.set_initial_max_path_id(0);
        }

        if no_cc {
            config.set_cc_algorithm(quiche::CongestionControlAlgorithm::DISABLED);
        }
        
        let h3_config = quiche::h3::Config::new().unwrap();

        let rng = SystemRandom::new();
        let conn_id_seed = ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();
        
        let mut next_client_id = 0;
        let mut clients_ids = ClientIdMap::new(); 
        let mut clients = ClientMap::new();

        let local_addr = socket.local_addr().unwrap();

        let resp = vec![
                    quiche::h3::Header::new(b":status", 200.to_string().as_bytes()),
                    quiche::h3::Header::new(b"server", b"quiche"),
                ];
        
        let mut continue_write = false;

        let app_data_start = std::time::Instant::now();
        let mut rcvd_time_map: HashMap<std::net::SocketAddr, Duration>  = HashMap::new();
        let mut last_ack_eliciting_sent_time = app_data_start.elapsed();

        let recvd_stream_id = 0;

        loop {
            // Find the shorter timeout from all the active connections.
            //
            // TODO: use event loop that properly supports timers
            let mut timeout = match continue_write {
                true => Some(Duration::from_millis(0)),

                false => {
                    if poll_timeout == 0 {
                        clients.values().filter_map(|c| c.conn.timeout()).min()
                    } else {
                        Some(Duration::from_millis(poll_timeout))
                    }
                },
            };
            poll.poll(&mut events, timeout).unwrap();

            // Read incoming UDP packets from the socket and feed them to quiche,
            // until there are no more packets to read.
            'read: loop {
                // If the event loop reported no events, it means that the timeout
                // has expired, so handle it without attempting to read packets. We
                // will then proceed with the send loop.
                if events.is_empty() && !continue_write {
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

                rcvd_time_map.insert(from, app_data_start.elapsed());

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
                let client = if !clients_ids.contains_key(&hdr.dcid) &&
                    !clients_ids.contains_key(&conn_id)
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
                        local_addr,
                        from,
                        &mut config,
                    )
                    .unwrap();

                    let client_id = next_client_id;

                    let client = QClient {
                        id: scid.clone(),
                        conn: Box::new(conn),
                        http3_conn: None,
                        client_id: client_id,
                        partial_responses: HashMap::new(),
                        sending_response: false,
                        max_datagram_size: MAX_DATAGRAM_SIZE,
                        loss_rate: 0.0,
                        max_send_burst: MAX_BUF_SIZE,
                        last_addr_recv: from,
                        last_primary_path: from,
                        cut_map: HashMap::new(),
                        rtt_map: HashMap::new(),
                    };

                    clients.insert(client_id, client);
                    clients_ids.insert(conn_id.clone(), client_id);

                    next_client_id += 1;

                    clients.get_mut(&client_id).unwrap()
                } else {
                    let cid = match clients_ids.get(&hdr.dcid) {
                        Some(v) => v,
    
                        None => clients_ids.get(&conn_id).unwrap(),
                    };
    
                    clients.get_mut(cid).unwrap()
                };

                let recv_info = quiche::RecvInfo {
                    to: local_addr,
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

                client.last_addr_recv = from;
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

                    let cid = match clients_ids.get(&conn_id) {
                        Some(v) => v,
                        None => clients_ids.get(&hdr.dcid).unwrap(),
                    };

                    // Create a new Client to handle the connection to gRPC.
                    match self.clients_tx.get(&cid) {
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
                                id: client.client_id.clone(),
                            };
            

                            // Let the client run on its own.
                            tokio::spawn(async move {
                                new_client.run().await.unwrap();
                            });
                            tokio::task::yield_now().await;
            
                            // Notify gRPC of the new client.
                            self.to_grpc.send(Ok(from_client))?;

                            self.clients_tx.insert(client.client_id.clone(), tx);
                            self.clients_tx.get_mut(&client.client_id.clone()).unwrap()


                        }
                    };
                    
                    // Update max_datagram_size after connection established.
                    client.max_datagram_size = client.conn.max_send_udp_payload_size();
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
                                    info!(
                                        "got {} bytes of data on stream {}",
                                        read, stream_id
                                    );
                                    //send data to gRPC
                                    let client_tx = match self.clients_tx.get(&client.client_id) {
                                        Some(v) => v,
                                        None => {
                                           error!("Client send data without proper http3 connection !");
                                           continue;
                                        }
                                    };
                                    match client_tx.send(buf[..read].to_vec()) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            println!("gRPC channel closed: {:?}", e);
                                            break;
                                        }
                                    }
                                    tokio::task::yield_now().await;

                                }
                            },

                            Ok((_stream_id, quiche::h3::Event::Finished)) => (),

                            Ok((_stream_id, quiche::h3::Event::Reset { .. })) => (),

                            Ok((_goaway_id, quiche::h3::Event::GoAway)) => (),

                            Ok((_, quiche::h3::Event::PriorityUpdate)) => (),

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

                let client = match clients.get_mut(&id) {
                    Some(v) => v,
                    None => {
                       //error
                       error!("Client not found in the client array !");      
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
                        //println!("-End of the gRPC response-");
                        client.sending_response = true;
                    }
                }
               
                Self::handle_path_events(client);
                    
                // See whether source Connection IDs have been retired.
                while let Some(retired_scid) = client.conn.retired_scid_next() {
                    info!("Retiring source CID {:?}", retired_scid);
                    clients_ids.remove(&retired_scid);
                }

                for path_id in client.conn.path_ids() {
                    // Provides as many CIDs as possible.
                    while client.conn.scids_left_on_path(path_id) > 0 {
                        let (scid, reset_token) = Self::generate_cid_and_reset_token(&rng);
                        if client
                            .conn
                            .new_scid_on_path(path_id, &scid, reset_token, false)
                            .is_err()
                        {
                            break;
                        }

                        clients_ids.insert(scid, client.client_id);
                    }
                } 
            }
            
            for client in clients.values_mut(){
                let mut conn = client.conn.as_mut();
                if(conn.is_multipath_enabled() && conn.is_established()) {
                    if app_data_start.elapsed() - last_ack_eliciting_sent_time > std::time::Duration::from_millis(ack_eliciting_timer) {
                        let mut scheduled_tuples = Self::scheduler_fn(&mut conn, "normal", &rcvd_time_map, &app_data_start, rcvd_threshold, &client.last_addr_recv, &client.last_primary_path, &mut client.cut_map, &mut client.rtt_map);
                        for (local_addr, peer_addr) in scheduled_tuples {
                            conn.send_ack_eliciting_on_path(local_addr, peer_addr).ok();
                        }
                        last_ack_eliciting_sent_time = app_data_start.elapsed();
                    }
                }
            }
            


            // Generate outgoing QUIC packets for all active connections and send
            // them on the UDP socket, until quiche reports that there are no more
            // packets to be sent.
            for client in clients.values_mut() {
                let mut conn = client.conn.as_mut();
                let last_addr_recv = client.last_addr_recv;
                let mut last_primary_path = client.last_primary_path;

                // Determine in which order we are going to iterate over paths.
                //let scheduled_tuples = lowest_latency_scheduler(&conn);
                let mut used_scheduler = scheduler;
                if !conn.is_established(){
                    used_scheduler = "normal";
                }

                let mut scheduled_tuples = Self::scheduler_fn(&mut conn, used_scheduler, &rcvd_time_map, &app_data_start, rcvd_threshold, &last_addr_recv, &last_primary_path, &mut client.cut_map, &mut client.rtt_map);

                // Generate outgoing QUIC packets and send them on the UDP socket, until
                // quiche reports that there are no more packets to be sent.
                if duplicate {
                    let mut n = 0;
                    let mut first_offset = 0;
                    let mut second_offset = 0;
                    let stream_id = 0;

                    let mut scheduled_tuples_duplicate = Self::scheduler_fn(&mut conn, used_scheduler, &rcvd_time_map, &app_data_start, rcvd_threshold, &last_addr_recv, &last_primary_path, &mut client.cut_map, &mut client.rtt_map);
                    
                    let (local_addr1, peer_addr1) = match scheduled_tuples_duplicate.next() {
                        Some((local_addr, peer_addr)) => (local_addr, peer_addr),
                        None => {
                            break
                        },
                    };
                    let (local_addr2, peer_addr2) = match scheduled_tuples_duplicate.next() {
                        Some((local_addr, peer_addr)) => (local_addr, peer_addr),
                        None => {
                            (local_addr1, peer_addr1)
                        },
                    };

                    let mut i = 0;
                    for (local_addr, peer_addr) in scheduled_tuples {
                        if i==0{
                            client.last_primary_path = peer_addr;
                        }
                        i += 1;

                        info!(
                            "sending on path ({}, {})",
                            local_addr, peer_addr
                        );
                        loop {
                            let stream = conn.streams.get(stream_id);
                            if !stream.is_none() {
                                first_offset = stream.unwrap().send.emit_off;
                            }

                            let (write, send_info) = match conn.send_on_path(
                                &mut out,
                                Some(local_addr),
                                Some(peer_addr),
                            ) {
                                Ok(v) => v,
        
                                Err(quiche::Error::Done) => {
                                    debug!("{} -> {}: done writing", local_addr, peer_addr);
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
                                            
                            // DUPLICATE ON PATH 
                            let stream = conn.streams.get_mut(stream_id);
                            if !stream.is_none() {
                                second_offset = stream.unwrap().send.emit_off;
                            }
                            let sent: usize = (second_offset - first_offset).try_into().unwrap();
                            if conn.paths.count_active_paths() > 1 && sent != 0 && second_offset > first_offset {
                                //indicate stream data to be retransmitted and flushable
                                let stream = match conn.streams.get_mut(stream_id) {
                                    Some(v) => v,
        
                                    None => continue,
                                };
        
                                let was_flushable = stream.is_flushable();
            
                                stream.send.retransmit(first_offset, sent);
        
                                // If the stream is now flushable push it to the
                                // flushable queue, but only if it wasn't already
                                // queued.
                                //
                                // Consider the stream flushable also when we are
                                // sending a zero-length frame that has the fin flag
                                // set.
                                if stream.is_flushable() && !was_flushable
                                {
                                    let priority_key = Arc::clone(&stream.priority_key);
                                    conn.streams.insert_flushable(&priority_key);
                                }
                                //needed ? 
                                /*
                                self.stream_retrans_bytes += length as u64;
                                p.stream_retrans_bytes += length as u64;
        
                                self.retrans_count += 1;
                                p.retrans_count += 1;
                                */
                                let (laddr, paddr) = Self::get_other_path(local_addr, peer_addr, &mut conn).unwrap();
        
                                info!(
                                    "sending duplicate on path ({}, {})",
                                    laddr, paddr
                                );
                                
                                let (write, send_info) = match conn.send_on_path(
                                    &mut out,
                                    Some(laddr),
                                    Some(paddr),
                                ) {
                                    Ok(v) => v,
            
                                    Err(quiche::Error::Done) => {println!{"Nothing left to write for path {:?}", laddr}; break},
            
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
                                            laddr,
                                            send_info.to
                                        );
                                        break;
                                    }
            
                                    panic!("send() failed: {:?}", e);
                                }
                    
                                info!("{} -> {}: duplicate written {}", laddr, send_info.to, write);
                                
                            }
                            
                            

                        }

                        n += 1;
                    }
                } else {
                    let mut i = 0;
                    for (local_addr, peer_addr) in scheduled_tuples {
                        i += 1;
                        if (i==1){
                            client.last_primary_path = peer_addr;
                        }
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
                                    debug!("{} -> {}: done writing", local_addr, peer_addr);
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
                            info!("{} -> {}: written {}", local_addr, send_info.to, write);
                        }
    
                    }
                    if i == 0{
                        println!("No path to send data !");
                    }
                }

            }


            // Garbage collect closed connections.
            clients.retain(|_, ref mut c| {
                info!("Collecting garbage");

                if c.conn.is_closed() {
                    println!(
                        "{} connection collected {:?} {:?}",
                        c.conn.trace_id(),
                        c.conn.stats(),
                        c.conn.path_stats().collect::<Vec<quiche::PathStats>>()
                    );

                    for id in c.conn.source_ids() {
                        let id_owned = id.clone().into_owned();
                        clients_ids.remove(&id_owned);
                }                
            }

                !c.conn.is_closed()
            });

            

            self.clients_tx.retain(|k, _| {
                clients.contains_key(k)
            });
        }
    }

    /// Generate a ordered list of 4-tuples on which the host should send packets,
    /// following a lowest-latency scheduling.
    fn scheduler_fn(
        conn: &mut quiche::Connection,
        scheduler: &str,
        rcvd_time_map: &HashMap<std::net::SocketAddr, Duration>,
        start: &std::time::Instant,
        threshold: u64,
        last_addr_recv: &std::net::SocketAddr,
        last_primary_path: &std::net::SocketAddr,
        cut_map: &mut HashMap<std::net::SocketAddr, bool>,
        rtt_map: &mut HashMap<std::net::SocketAddr, std::time::Duration>,
    ) -> impl Iterator<Item = (std::net::SocketAddr, std::net::SocketAddr)> {
        if scheduler == "lrtt" {
            //lowest-rtt-first scheduler
            use itertools::Itertools;
            let mut paths: Vec<_> = conn
                .path_stats()
                .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
                .sorted_by_key(|p| {info!("path {:?} with RTT : {:?}", p, p.rtt); p.rtt})
                .map(|p| (p.local_addr, p.peer_addr))
                .collect();

            paths.into_iter()
        } else if scheduler == "lrtt2" { 
            //lowest-rtt-first scheduler improved
            use itertools::Itertools;
            let mut primary_path = None;
            let mut i = 0;
            let mut cutted = false;
            let mut paths: Vec<_> = conn
                .path_stats()
                .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
                .sorted_by_key(|p| {
                    if let Some(time) = rcvd_time_map.get(&p.peer_addr) {
                        let n = start.elapsed() - *time;
                        //PTO calculation
                        let thres = p.rtt + std::cmp::max(4*p.rttvar, std::time::Duration::from_millis(1))  + std::time::Duration::from_millis(threshold);
                        if n > thres{
                            if (p.peer_addr == *last_addr_recv){
                                cutted = true;
                            }
                            cut_map.insert(p.peer_addr, true);
                            rtt_map.insert(p.peer_addr, p.rtt);
                            std::time::Duration::from_secs(u64::MAX)
                        } else if *cut_map.get(&p.peer_addr).unwrap_or_else(|| &false) {
                            if p.rtt == *rtt_map.get(&p.peer_addr).unwrap() {
                                std::time::Duration::from_secs(u64::MAX)
                            } else {
                                cut_map.insert(p.peer_addr, false);
                                rtt_map.insert(p.peer_addr, p.rtt);
                                p.rtt
                            }
                        } else {
                            rtt_map.insert(p.peer_addr, p.rtt);
                            p.rtt
                        }
                    } else {
                        rtt_map.insert(p.peer_addr, p.rtt);
                        p.rtt
                    }
                })
                .map(|p| {
                    if i == 0 {
                        primary_path = Some(p.peer_addr);
                    }
                    i += 1;
                
                    (p.local_addr, p.peer_addr)})
                    .collect();

            //mark all packets in flight as lost
            if cutted && conn.is_established() && !primary_path.is_none() && primary_path != Some(*last_primary_path) {
                let path_id = conn.path_stats().find(|p| p.peer_addr == *last_primary_path).unwrap().path_id;
                println!("Marking all packets from path {:?} as lost", path_id);
                conn.paths.get_mut(path_id.try_into().unwrap()).unwrap().recovery.mark_all_inflight_as_lost(std::time::Instant::now(), " ");
            }
    
            paths.into_iter()
        } else if scheduler == "normal" {
            use itertools::Itertools;
            let mut paths: Vec<_> = conn
                .path_stats()
                .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
                .map(|p| {info!("path : {:?}", p); (p.local_addr, p.peer_addr)})
                .collect();
    
            paths.into_iter()
        } else if scheduler == "last" {
            use itertools::Itertools;
            let mut paths: Vec<_> = conn
                .path_stats()
                .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
                .sorted_by_key(|p| {
                    if p.peer_addr == *last_addr_recv {
                        0
                    } else {
                        1
                    }
                })
                .map(|p| {info!("path : {:?}", p); (p.local_addr, p.peer_addr)})
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

    fn get_other_path(src: std::net::SocketAddr, dest: std::net::SocketAddr, conn: &mut quiche::Connection,)
    -> Option<(std::net::SocketAddr, std::net::SocketAddr)> {
        conn.path_stats()
            .filter(|p| !matches!(p.state, quiche::PathState::Closed(_)))
            .find(|p| p.local_addr == src && p.peer_addr != dest)
            .map(|p| {(p.local_addr, p.peer_addr)})
    }

    fn handle_path_events(client: &mut QClient) {
        while let Some((pid, qe)) = client.conn.path_event_next() {
            match qe {
                quiche::PathEvent::New(local_addr, peer_addr) => {
                    println!(
                        "{} Seen new path ({}, {}) with ID {}",
                        client.conn.trace_id(),
                        local_addr,
                        peer_addr,
                        pid
                    );
    
                    // Directly probe the new path.
                    client
                        .conn
                        .probe_path(local_addr, peer_addr)
                        .map_err(|e| error!("cannot probe: {}", e))
                        .ok();
                },
    
                quiche::PathEvent::Validated(local_addr, peer_addr) => {
                    println!(
                        "{} Path ({}, {}) with ID {} is now validated",
                        client.conn.trace_id(),
                        local_addr,
                        peer_addr,
                        pid
                    );
                    if client.conn.is_multipath_enabled() {
                        client
                            .conn
                            .set_active(local_addr, peer_addr, true)
                            .map_err(|e| error!("cannot set path active: {}", e))
                            .ok();
                    }
                },
    
                quiche::PathEvent::FailedValidation(local_addr, peer_addr) => {
                    println!(
                        "{} Path ({}, {}) with ID {} failed validation",
                        client.conn.trace_id(),
                        local_addr,
                        peer_addr,
                        pid
                    );
                },
    
                quiche::PathEvent::Closed(local_addr, peer_addr, err) => {
                    println!(
                        "{} Path ({}, {}) with ID {} is now closed and unusable; err = {}",
                        client.conn.trace_id(),
                        local_addr,
                        peer_addr,
                        pid,
                        err,
                    );
                },
    
                quiche::PathEvent::ReusedSourceConnectionId(cid_seq, old, new) => {
                    println!(
                        "{} Peer reused cid seq {} (initially {:?}) on {:?} on path ID {}",
                        client.conn.trace_id(),
                        cid_seq,
                        old,
                        new,
                        pid
                    );
                },
    
                quiche::PathEvent::PeerMigrated(local_addr, peer_addr) => {
                    println!(
                        "{} Connection migrated to ({}, {}) on Path ID {}",
                        client.conn.trace_id(),
                        local_addr,
                        peer_addr,
                        pid
                    );
                },
    
                quiche::PathEvent::PeerPathStatus(addr, path_status) => {
                    println!(
                        "Peer asks status {:?} for {:?} on path ID {}",
                        path_status, addr, pid
                    );
                    client
                        .conn
                        .set_path_status(addr.0, addr.1, path_status, false)
                        .map_err(|e| error!("cannot follow status request: {}", e))
                        .ok();
                },
            }
        }
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
                //println!("-End of the gRPC response-");
                client.sending_response = false;
            }

            client.partial_responses.remove(&stream_id);
        } else {
            info!("{} stream {} written {} bytes", conn.trace_id(), stream_id, written);
        }
    }

    pub fn send_to(
        socket: &mio::net::UdpSocket, buf: &[u8], send_info: &quiche::SendInfo,
        segment_size: usize,
    ) -> std::io::Result<usize> {
        let mut off = 0;
        let mut left = buf.len();
        let mut written = 0;

        while left > 0 {
            let pkt_len = std::cmp::min(left, segment_size);

            match socket.send_to(&buf[off..off + pkt_len], send_info.to) {
                Ok(v) => {
                    written += v;
                },
                Err(e) => return Err(e),
            }

            off += pkt_len;
            left -= pkt_len;
        }

        Ok(written)
    }
}

struct Client {
    stream: DuplexStream,
    to_io: UnboundedSender<(Vec<u8>, ClientId)>,
    from_io: UnboundedReceiver<Vec<u8>>,
    id: ClientId,
}

impl Client {
    async fn run(&mut self) -> Result<()> {
        let mut time_amount = 0;
        let mut buf = [0u8; MAX_BRIDGING_BUFFER_SIZE];
        let mut grpc_up = false;
        loop {
            tokio::select! {
                Some(msg) = self.from_io.recv() => self.handle_io_msg(msg).await?,
                Ok(len) = self.stream.read(&mut buf[..]) => {
                    if len == 0 && grpc_up {
                        return Ok(());
                    }
                    grpc_up = true;
                    self.handle_grpc_msg(&buf[..len]).await?
                },
            }
            if self.from_io.is_closed() {
                return Ok(());
            }

        }
        
    }

    async fn handle_io_msg(&mut self, msg: Vec<u8>) -> Result<()> {
        self.stream.write(&msg).await?;
        Ok(())
    }

    async fn handle_grpc_msg(&mut self, msg: &[u8]) -> Result<()> {
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

    } else if proto == "filetransfer" || proto == "fileexchange" {
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

    async fn exchange_file(
        &self,
        request: Request<FileData>,
    ) -> std::result::Result<Response<FileData>, Status> {
        let file = request.into_inner();
        let mut new_file = File::create(file.filename.clone()).await.map_err(|e| Status::internal(e.to_string()))?;
        new_file.write_all(&file.data).await.map_err(|e| Status::internal(e.to_string()))?;

        let filename = file.filename;
        let mut file_down = File::open(&filename).await.map_err(|e| Status::not_found(e.to_string()))?;
        let mut buffer = Vec::new();
        file_down.read_to_end(&mut buffer).await.map_err(|e| Status::internal(e.to_string()))?;

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

