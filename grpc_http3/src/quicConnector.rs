//use tonic::transport::{Channel};
use tower_service;
use std::task::{Context, Poll};
use std::sync::{Arc, Mutex};
use std::pin::Pin;
use tonic::transport::Uri;
use hyper::rt::{Write,Read};
use hyper::rt::ReadBufCursor;
use std::io::Error;
use futures_util::future::{self,Ready};
use tokio::task;
use ring::rand::*;
use log::info;
use log::debug;
use log::error;
use std::time::Duration;
use quiche::h3::NameValue;
//use std::sync::mpsc::{Sender,Receiver};
use flume::{Sender, Receiver};
use tokio::io::ErrorKind;

//use future_bool::FutureBool;


const MAX_DATAGRAM_SIZE: usize = 1350;

const HTTP_REQ_STREAM_ID: u64 = 4;

pub struct QuicConnector {
    //define attributes for the connector, it's the channel to send data
    senderCS: Sender<Vec<u8>>,
    receiverSC: Receiver<Vec<u8>>,
    //receiverCS: Receiver<Vec<u8>>,
    //senderSC: Sender<Vec<u8>>,
    //receiverSC: Receiver<Vec<u8>>,
    //ready: FutureBool, //may be useful to notifiy when the QUIC connection is ready
}


impl QuicConnector {
    
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        //let (senderCS, receiverCS) = std::sync::mpsc::channel::<Vec<u8>>(); //channel to send data from client to server
        let (senderCS, receiverCS) = flume::unbounded::<Vec<u8>>();  //channel to send data from client to server
        let (senderSC, receiverSC) = flume::unbounded::<Vec<u8>>();  //channel to receive response from server

        //let ready = FutureBool::new(false);
        //let ready_clone = ready.clone();

        let receiverCS_ref = Arc::new(Mutex::new(receiverCS));

        task::spawn_blocking(move || {
            let mut buf = [0; 65535];
            let mut out = [0; MAX_DATAGRAM_SIZE];
        
            
        
            let endpoint = match std::env::var_os("HTTPBIN_ENDPOINT") {
                Some(val) => val.into_string().unwrap(),
    
                None => String::from("https://127.0.0.1:4433"),
            };

            println!("Connection to {} ", endpoint); 
    
            let url: url::Url = url::Url::parse(&endpoint).unwrap();

        
            // Setup the event loop.
            let mut poll = mio::Poll::new().unwrap();
            let mut events = mio::Events::with_capacity(1024);
        
            // Resolve server address.
            //let peer_addr = url.socket_addrs(|| None).unwrap()[0];
            let peer_addr: std::net::SocketAddr = "127.0.0.1:4433".parse().expect("Failed to parse address");
        
            // Bind to INADDR_ANY or IN6ADDR_ANY depending on the IP family of the
            // server address. This is needed on macOS and BSD variants that don't
            // support binding to IN6ADDR_ANY for both v4 and v6.
            let bind_addr = match peer_addr {
                std::net::SocketAddr::V4(_) => "0.0.0.0:0",
                std::net::SocketAddr::V6(_) => "[::]:0",
            };
        
            // Create the UDP socket backing the QUIC connection, and register it with
            // the event loop.
            let mut socket =
                mio::net::UdpSocket::bind(bind_addr.parse().unwrap()).unwrap();
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
        
            let mut http3_conn = None;
        
            // Generate a random source connection ID for the connection.
            let mut scid = [0; quiche::MAX_CONN_ID_LEN];
            SystemRandom::new().fill(&mut scid[..]).unwrap();
        
            let scid = quiche::ConnectionId::from_ref(&scid);
        
            // Get local address.
            let local_addr = socket.local_addr().unwrap();
        
            // Create a QUIC connection and initiate handshake.
            let mut conn =
                quiche::connect(url.domain(), &scid, local_addr, peer_addr, &mut config)
                    .unwrap();
        
            info!(
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
        
            // Prepare request.
            let mut path = String::from(url.path());
        
            if let Some(query) = url.query() {
                path.push('?');
                path.push_str(query);
            }
        
            let req = vec![
                quiche::h3::Header::new(b":method", b"POST"),
                quiche::h3::Header::new(b":scheme", url.scheme().as_bytes()),
                quiche::h3::Header::new(
                    b":authority",
                    url.host_str().unwrap().as_bytes(),
                ),
                quiche::h3::Header::new(b":path", path.as_bytes()),
                quiche::h3::Header::new(b"user-agent", b"quiche"),
            ];
        
            let req_start = std::time::Instant::now();
        
            let mut req_sent = false;
            let mut i = 0;
        
            loop {
                //println!("loop {}", i);
                poll.poll(&mut events, conn.timeout()).unwrap();
        
                // Read incoming UDP packets from the socket and feed them to quiche,
                // until there are no more packets to read.
                'read: loop {
                    // If the event loop reported no events, it means that the timeout
                    // has expired, so handle it without attempting to read packets. We
                    // will then proceed with the send loop.
                    if events.is_empty() {
                        debug!("timed out");
        
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
        
                            panic!("recv() failed: {:?}", e);
                        },
                    };
        
                    debug!("got {} bytes", len);
        
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
        
                    debug!("processed {} bytes", read);
                }
        
                debug!("done reading");
        
                if conn.is_closed() {
                    println!("connection closed, {:?}", conn.stats());
                    break;
                }
        
                // Create a new HTTP/3 connection once the QUIC connection is established.
                if conn.is_established() && http3_conn.is_none() {
                    //The connection is established, send message to tonic to notify the connection is ready
                    //ready_clone.set(true);

                    http3_conn = Some(
                        quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                        .expect("Unable to create HTTP/3 connection, check the server's uni stream limit and window size"),
                    );
                }
        
                // Send HTTP requests once the QUIC connection is established, and until
                // all requests have been sent.
                if let Some(h3_conn) = &mut http3_conn {
                    /* 
                    if !req_sent {
                        println!("sending HTTP request {:?}", req);
        
                        h3_conn.send_request(&mut conn, &req, true).unwrap();
        
                        req_sent = true;
                    }*/
                    /* 
                    while let Ok(data) = receiverCS_ref.lock().unwwrap().try_recv() {
                        //println!("sending HTTP request {:?}", req);
                        h3_conn.send_request(&mut conn, &req, true).unwrap();
                        h3_conn.send_body(&mut conn, HTTP_REQ_STREAM_ID, &data, true).unwrap();
                    }*/

                    loop {
                        match receiverCS_ref.lock().unwrap().try_recv() {
                            Ok(msg) => {println!("{:?}", msg); 
                                    //println!("Data size: {} bytes", size_of_val(&*msg));
                            
                                    let req = vec![
                                        quiche::h3::Header::new(b":method", b"GET"),
                                        quiche::h3::Header::new(b":scheme", b"https"),
                                        quiche::h3::Header::new(b":authority", b"quic.tech"),
                                        quiche::h3::Header::new(b":path", b"/"),
                                        quiche::h3::Header::new(b"user-agent", b"quiche"),
                                        quiche::h3::Header::new(
                                            b"content-length",
                                            size_of_val(&*msg).to_string().as_bytes(),
                                        ),
                                    ];

                                    println!("sending HTTP request {:?}", req);
                                    let stream_id = h3_conn.send_request(&mut conn, &req, false).unwrap();
                                    h3_conn.send_body(&mut conn, stream_id, &msg, true);
                                }
            
                            Err(_) => {println!("breaking this loop"); break;}
                        }
                    }
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
        
                                    println!("response is {}", unsafe {
                                        std::str::from_utf8_unchecked(&buf[..read])
                                    });
                                    senderSC.send(buf[..read].to_vec()).unwrap();
                                }
                            },
        
                            Ok((_stream_id, quiche::h3::Event::Finished)) => {
                                println!(
                                    "response received in {:?}, closing...",
                                    req_start.elapsed()
                                );
        
                                //conn.close(true, 0x100, b"kthxbye").unwrap();
                            },
        
                            Ok((_stream_id, quiche::h3::Event::Reset(e))) => {
                                error!(
                                    "request was reset by peer with {}, closing...",
                                    e
                                );
        
                                //conn.close(true, 0x100, b"kthxbye").unwrap();
                            },
        
                            Ok((_, quiche::h3::Event::PriorityUpdate)) => unreachable!(),
        
                            Ok((goaway_id, quiche::h3::Event::GoAway)) => {
                                info!("GOAWAY id={}", goaway_id);
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
                            debug!("done writing");
                            break;
                        },
        
                        Err(e) => {
                            error!("send failed: {:?}", e);
        
                           //conn.close(false, 0x1, b"fail").ok();
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
        
                if conn.is_closed() {
                    println!("connection closed, {:?}", conn.stats());
                    break;
                }
                i = i + 1;
            }     
            
        });


        Ok(QuicConnector {senderCS, receiverSC})
    }
}

impl tower_service::Service<Uri> for QuicConnector {
    type Response = HTTP3Connection;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        //determines if the connector is ready.
        //no specific conditions here so always ready.
        Poll::Ready(Ok(()))
        //Poll::Pending
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        // return a futur when the connection is ready
        future::ready(Ok(HTTP3Connection::new(self.senderCS.clone(), self.receiverSC.clone())))
    }
}

pub struct HTTP3Connection {
    //connector: QuicConnector,
    sender: Sender<Vec<u8>>,
    receiver: Receiver<Vec<u8>>,
}

impl HTTP3Connection {
    pub fn new(sender: Sender<Vec<u8>>, receiver: Receiver<Vec<u8>>) -> Self {
        HTTP3Connection {
            //connector: connector,
            sender: sender,
            receiver: receiver,
            //receiver: receiver,
        }
    }

    pub fn write_to_channel(&mut self, buf: &[u8]) -> Result<usize, Error> {
        // Convert the buffer to a Vec<u8> since the Sender requires owned data
        let data = buf.to_vec();

        // Try to send the data through the channel
        match self.sender.send(data) {
            Ok(_) => Ok(buf.len()), // Successfully sent all bytes to the QUIC thread
            Err(_) => Err(Error::new(ErrorKind::BrokenPipe, "Channel closed")),
        }
    }
}


impl Read for HTTP3Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), Error>> {
        // READ PACKETS
        //Poll::Pending
        println!("Pending read");
        
        let five_seconds = Duration::new(5, 0);
        let msg = self.receiver.recv_timeout(five_seconds);
        match msg {
            Ok(data) => {
                println!("Received data: {:?}", data);
                buf.put_slice(&data);
                Poll::Ready(Ok(()))
            },
            Err(e) => {
                println!("Error receiving data: {:?}", e);
                //Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e)))
                Poll::Pending
            }
        } 
       //Poll::Pending
    }
}

impl Write for HTTP3Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        // WRITTE PACKETS
       // Poll::Pending
       let this = self.get_mut();

       // Attempt to write data to your channel
       match this.write_to_channel(buf) {
           Ok(n) => Poll::Ready(Ok(n)),
           /* 
           Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
               // If the channel is full, register the waker and return Pending
               this.register_waker(cx.waker().clone());
               Poll::Pending
           }*/
           Err(e) => Poll::Ready(Err(e)),
    }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        // Not needed
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        // Close the gracefully the connection
        Poll::Ready(Ok(()))
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