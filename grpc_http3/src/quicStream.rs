use std::task::{Context, Poll};
use std::pin::Pin;
use std::net::SocketAddr;
use quiche::ConnectionId;
use futures_core::stream::Stream;
use tokio::io::{AsyncRead, AsyncWrite};
use std::sync::{Arc, Mutex};
use tonic::transport::server::Connected;
use std::io::Error;
use tokio::task;
use log::info;
use log::debug;
use log::error;
use log::warn;
use log::trace;
use futures_util::Future;
use std::time::Duration;
use std::thread;
//use std::task::Waker;
//use futures_util::task::Waker;
use futures_util::task::Waker;

use std::net;

use std::collections::HashMap;

use ring::rand::*;
use quiche::h3::NameValue;

const MAX_DATAGRAM_SIZE: usize = 1350;

struct PartialResponse {
    headers: Option<Vec<quiche::h3::Header>>,

    body: Vec<u8>,

    written: usize,
}

struct Client {
    conn: quiche::Connection,

    http3_conn: Option<quiche::h3::Connection>,

    partial_responses: HashMap<u64, PartialResponse>,
}

type ClientMap = HashMap<quiche::ConnectionId<'static>, Client>;

/*
    QuicStream is an async stream representings a streams of QUIC connections.
*/
pub struct QuicStream {
   receiverCS: Arc<Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
   senderSC: flume::Sender<Vec<u8>>,
   n: i32,
   //shared_state: Arc<Mutex<SharedState>>,
}

impl QuicStream {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        
       //let (senderCS, receiverCS) = std::sync::mpsc::channel::<Vec<u8>>(); //channel to send data from client to server
       //let (senderCS, receiverCS) = flume::unbounded::<Vec<u8>>();
       let (senderCS, receiverCS) = tokio::sync::mpsc::channel::<Vec<u8>>(100); //Channel from Quiche Thread to the IO thread
       let (senderSC, receiverSC) = flume::unbounded::<Vec<u8>>();  // Channel from IO thread to the Quiche Thread
       let receiverCS_ref = Arc::new(Mutex::new(receiverCS));

       // Threads that handle the QUIC connection and send HTTP/3 responses to requests.
       task::spawn_blocking(move || {
        let mut buf = [0; 65535];
        let mut out = [0; MAX_DATAGRAM_SIZE];

        // Setup the event loop.
        let mut poll = mio::Poll::new().unwrap();
        let mut events = mio::Events::with_capacity(1024);
    
        // Create the UDP listening socket, and register it with the event loop.
        let mut socket =
            mio::net::UdpSocket::bind("127.0.0.1:4433".parse().unwrap()).unwrap();
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
        config.enable_early_data();
    
        let h3_config = quiche::h3::Config::new().unwrap();
    
        let rng = SystemRandom::new();
        let conn_id_seed =
            ring::hmac::Key::generate(ring::hmac::HMAC_SHA256, &rng).unwrap();
    
        let mut clients = ClientMap::new();
    
        let local_addr = socket.local_addr().unwrap();
        let mut i = 0;
        loop {
            //println!("loop {}", i);
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
    
                        panic!("recv() failed: {:?}", e);
                    },
                };
    
                debug!("got {} bytes", len);
    
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
    
                debug!("got packet {:?}", hdr);
    
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
                        println!("Doing version negotiation");
    
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
                        println!("Doing stateless retry");
    
                        let new_token = mint_token(&hdr, &from);
    
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
    
                    let odcid = validate_token(&from, token);
    
                    // The token was not valid, meaning the retry failed, so
                    // drop the packet.
                    if odcid.is_none() {
                        println!("Invalid address validation token");
                        continue 'read;
                    }
    
                    if scid.len() != hdr.dcid.len() {
                        println!("Invalid destination connection ID");
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
    
                    let client = Client {
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
    
                debug!("{} processed {} bytes", client.conn.trace_id(), read);
    
                // Create a new HTTP/3 connection as soon as the QUIC connection
                // is established.

                //println!("connection established ? {}", client.conn.is_established()); 
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
                }
    
                if client.http3_conn.is_some() {
                    // Handle writable streams.
                    for stream_id in client.conn.writable() {
                        handle_writable(client, stream_id);
                    }
    
                    // Process HTTP/3 events.
                    loop {
                        let http3_conn = client.http3_conn.as_mut().unwrap();
    
                        match http3_conn.poll(&mut client.conn) {
                            Ok((
                                stream_id,
                                quiche::h3::Event::Headers { list, .. },
                            )) => {
                                /*handle_request(
                                    client,
                                    stream_id,
                                    &list,
                                    "examples/root",
                                    receiverSC.clone(),
                                );*/
                            },
    
                            Ok((stream_id, quiche::h3::Event::Data)) => {
                                println!(
                                    "{} got data on stream id {}",
                                    client.conn.trace_id(),
                                    stream_id
                                );

                                let mut body = vec![0; 4096];

                                // Consume all body data received on the stream.
                                while let Ok(read) = http3_conn.recv_body(&mut client.conn, stream_id, &mut body) {
                                    println!("Received {} bytes of payload on stream {}", read, stream_id);
                                    //println!("Payload: {:?}", &body[..read]);
                                    let data: Vec<u8> = body[..read].to_vec();
                                    //println!("Disconnected ? {}", senderCS.is_disconnected());
                                    let result = match senderCS.try_send(data) {
                                        Ok(_) => {
                                            //println!("Data sent successfully");
                                        },
                                        Err(e) => {
                                            println!("Error sending data: {:?}", e.to_string());
                                        }
                                    };


                                    //println!("Result a: {:?}", result.err().into_iter());
                                }

                                loop {
                                    println!("Trying to send data");
                                    match receiverSC.try_recv() {
                                        Ok(msg) => {println!("Sending response : {:?}", msg); 
                                                
                                                println!("Data size: {} bytes", size_of_val(&*msg));
                                        
                                                let req = vec![
                                                    quiche::h3::Header::new(b":status", b"200"),
                                                    quiche::h3::Header::new(b"server", b"quiche"),
                                                    quiche::h3::Header::new(
                                                        b"content-length",
                                                        msg.len().to_string().as_bytes(),
                                                    ),
                                                ];
                                                println!("sending HTTP request {:?}", req);
                                                http3_conn.send_response(&mut client.conn, stream_id, &req, false) ;
                                                http3_conn.send_body(&mut client.conn, stream_id, &msg, true);
                                            }
                        
                                        Err(_) => {println!("nothing to send"); break;}
                                    }
                                } 


                            },
    
                            Ok((_stream_id, quiche::h3::Event::Finished)) => (),
    
                            Ok((_stream_id, quiche::h3::Event::Reset { .. })) => (),
    
                            Ok((
                                _prioritized_element_id,
                                quiche::h3::Event::PriorityUpdate,
                            )) => (),
    
                            Ok((_goaway_id, quiche::h3::Event::GoAway)) => (),
    
                            Err(quiche::h3::Error::Done) => {
                                //println!("{} done reading", client.conn.trace_id());
                                break;
                            },
    
                            Err(e) => {
                                error!(
                                    "{} HTTP/3 error {:?}",
                                    client.conn.trace_id(),
                                    e
                                );
                                println!("HTTP/3 error: {:?}", e);
    
                                break;
                            },
                        }
                                              
          
                    }
                    /* 
                    loop {
                        let http3_conn = client.http3_conn.as_mut().unwrap();
                        println!("Trying to send data");
                        match receiverSC.try_recv() {
                            Ok(msg) => {println!("Sending response : {:?}", msg); 
                                    
                                    println!("Data size: {} bytes", size_of_val(&*msg));
                            
                                    let req = vec![
                                        quiche::h3::Header::new(b":status", b"200"),
                                        quiche::h3::Header::new(b"server", b"quiche"),
                                        quiche::h3::Header::new(
                                            b"content-length",
                                            msg.len().to_string().as_bytes(),
                                        ),
                                    ];
                                    println!("sending HTTP request {:?}", req);
                                    let stream_id = http3_conn.send_request(&mut client.conn, &req, false).unwrap();
                                    http3_conn.send_body(&mut client.conn, stream_id, &msg, true);
                                }
            
                            Err(_) => {println!("nothing to send"); break;}
                        }
                    } */
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
                            //println!("{} done writing", client.conn.trace_id());
                            break;
                        },
    
                        Err(e) => {
                            error!("{} send failed: {:?}", client.conn.trace_id(), e);
                            println!("send failed: {:?}", e);
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
                debug!("Collecting garbage");
    
                if c.conn.is_closed() {
                    println!(
                        "{} connection collected {:?}",
                        c.conn.trace_id(),
                        c.conn.stats()
                    );
                }
    
                !c.conn.is_closed()
            });
            i = i + 1;
        }
            
        });
    
    
        /*
        let shared_state = Arc::new(Mutex::new(SharedState {
            completed: false,
            working: false,
            waker: None,
        }));
    
        let receiver = receiverCS.clone();
        let dur = Duration::from_millis(30);
        let thread_shared_state = shared_state.clone();
        
        
        thread::spawn(move || {
        
            loop {
                thread::sleep(dur);
                let mut shared_state = thread_shared_state.lock().unwrap();
                // Signal that the timer has completed and wake up the last
                // task on which the future was polled, if one exists.
                if(!receiver.is_disconnected() && !receiver.is_empty() && !shared_state.completed && !shared_state.working){
                    println!("Data is available !");
                    shared_state.completed = true;
                    shared_state.working = true;
                
                    if let Some(waker) = shared_state.waker.take() {
                        println!("Waking up the task !");
                        waker.wake_by_ref()
                       
                    }
                }
            }
        });   */

        
        //Ok(QuicStream {receiverCS_ref, n: 0})
        Ok(QuicStream {receiverCS: receiverCS_ref.clone(), senderSC: senderSC.clone(),n: 0})
    }
}

/*
    The Stream trait is implemented for QuicStream.
    This trait is needed the tonic::transport::server::Route::serve_with_incoming() function
*/
impl Stream for QuicStream{
    type Item = Result<IO, Error>;

    // Required method
    /*
        Returns the next value in the stream.
        This functions returns a Future containing a IO object representing a new QUIC Connection.
        TODO: ADD the handling of multiple QUIC Connections.
        --> Currently, only returns one connection and then Poll:Pending
    */
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>
    ) -> Poll<Option<Self::Item>>{
        println!("Polling a new connection");
        if(self.n == 0){
            self.n = self.n + 1;
            //Poll::Ready(Some(Ok(IO::new(self.receiverCS_ref.clone()))))
            Poll::Ready(Some(Ok(IO::new(self.receiverCS.clone(), self.senderSC.clone()))))
        }else{
            Poll::Ready(None)
        }  
        /*
        let mut shared_state = self.shared_state.lock().unwrap();
        println!("the task is up !");
        if(shared_state.completed){
            println!("Data is available");
            shared_state.completed = false;
            Poll::Ready(Some(Ok(IO::new(self.receiverCS.clone(), self.senderSC.clone(), self.shared_state.clone()))))
        } else {
            println!("must wait");
            shared_state.waker = Some(cx.waker().clone());
            Poll::Pending
        } */
        

        
    }
}

/*
    This structure represents a QUIC Connection.
    It implements the AsyncRead and AsyncWrite traits which allows to read and write data from/to the HTTP/3 traffic.
*/
pub struct IO {
    //define attributes for the IO
    //receiverCS_ref:  Arc<Mutex<std::sync::mpsc::Receiver<Vec<u8>>>>,
    receiverCS:  Arc<Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    senderSC: flume::Sender<Vec<u8>>,
}

/*
    Structure used to share the state with a thread that will wake up a task.
*/
struct SharedState {
    completed: bool,
    waker: Option<Waker>,
}

impl IO {
    pub fn new (receiverCS: Arc<Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>, senderSC: flume::Sender<Vec<u8>>,) -> Self {
        println!("Creating new IO");
        //println!("ReceiverCS is disconnected ? {}", receiverCS.is_disconnected());
/* 
        let shared_read_state = Arc::new(Mutex::new(SharedState {
            completed: false,
            waker: None,
        }));
 
        let one_second = Duration::new(1, 0);

        let thread_shared_read_state = shared_read_state.clone();
        let receiver = receiverCS.clone();
        
        thread::spawn(move || {
            loop {
                //thread::sleep(one_second);
                let mut shared_state = thread_shared_read_state.lock().unwrap();
                // Signal that the timer has completed and wake up the last
                // task on which the future was polled, if one exists.
                if(!receiver.is_empty()){
                    println!("Data is available");
                    shared_state.completed = true;
                    
                    if let Some(waker) = shared_state.waker.take() {
                        println!("Waking up the task");
                        waker.wake_by_ref()
                       
                    }
                    break;
                }
                drop(shared_state);
            }
        });  */

        IO {receiverCS, senderSC}
        
    }
}

impl AsyncRead for IO {
    /*
        Puts data receive from a HTTP/3 requests into a buffer.
        Returns Poll:Pending if there is currently no data to read.
        PROBLEM: The task is never woken up when there is data to read.
    */
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf
    ) -> Poll<Result<(), std::io::Error>> {
        // READ PACKETS
        println!("Polling the read");  
        /* 
        // Poll the receiver and put data into the buffer -> directly modifies the result in the Poll
        let pol = self.receiverCS.lock().unwrap().poll_recv(cx);
        pol.map(|msg| {
            let data = msg.unwrap();
            println!("Received data: {:?}", data);
            buf.put_slice(&data);
            Ok(())
        })
        */

        
        // Try to receive data. If nothing is available, return Poll::Pending and spawn a thread that will wake up the task when there is data to read.
        let msg = self.receiverCS.lock().unwrap().try_recv();
        match msg {
            Ok(data) => {
                println!("Received data: {:?}", data);
                buf.put_slice(&data);
                Poll::Ready(Ok(()))
            },
            Err(e) => {
                println!("must wait");
                //create threads that will wake up the task when there is something to read
                let receiver = self.receiverCS.clone();
                let waker = cx.waker().clone();
                let timer = Duration::from_millis(25);
                thread::spawn(move || {
                    loop {
                        thread::sleep(timer);
                        // Signal that the timer has completed and wake up the last
                        // task on which the future was polled, if one exists.
                        if(!receiver.lock().unwrap().is_empty()){
                            println!("Data is available");
                            waker.wake();            
                            break;
                        }
                    }
                });  

                Poll::Pending
            }
        } 

        /* 
        let msg = self.receiverCS.recv();
        match msg {
            Ok(data) => {
                println!("Received data: {:?}", data);
                buf.put_slice(&data);
                Poll::Ready(Ok(()))
            },
            Err(e) => {
                println!("Error receiving data: {:?}", e);
                Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e)))
            }
        }*/
        
        

        /*
        let future = self.receiverCS.recv_async();
        let mapped = future.map(|data| {
            println!("Received data: {:?}", data);
            buf.put_slice(&data);
            Ok(())
        });
        mapped.map_err(|e| {
            println!("Error receiving data: {:?}", e);
            std::io::Error::new(std::io::ErrorKind::Other, e)
        }).poll(cx) */

        /* 
        let mut shared_state = self.shared_read_state.lock().unwrap();
        println!("the task is up !");
        println!("Data is available ? {}", shared_state.completed);
        if(shared_state.completed){
            println!("Data is available");
            let msg = self.receiverCS.try_recv();
            match msg {
                Ok(data) => {
                    println!("Received data: {:?}", data);
                    buf.put_slice(&data);
                    drop(shared_state);
                    Poll::Ready(Ok(()))
                },
                Err(e) => {
                    //println!("Error receiving data: {:?}", e);
                    println!("must wait");
                    shared_state.waker = Some(cx.waker().clone());
                    cx.waker().wake_by_ref();
                    drop(shared_state);
                    Poll::Pending
                }
            }
    
        } else {
            println!("must wait");
            shared_state.waker = Some(cx.waker().clone());
            cx.waker().wake_by_ref();
            drop(shared_state);
            Poll::Pending
        } */
        
        

        /* 
        //Dummy implementation that waits 5 seconds and try to read data.
        //If there is nothing returns that all the reading is done.
        // let five_seconds = Duration::new(5, 0);
        //let msg = self.receiverCS.recv_timeout(five_seconds);
        let msg = self.receiverCS.try_recv();
        match msg {
            Ok(data) => {
                println!("Received data: {:?}", data);
                buf.put_slice(&data);
                Poll::Ready(Ok(()))
            },
            Err(e) => {
                println!("Error receiving data: {:?}", e);
                //Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, e)))
                self.shared_state.lock().unwrap().working = false;
                Poll::Ready(Ok(()))
            }
        } */

       
                
        
    }
}

impl AsyncWrite for IO {

    /*
       Puts data in the channel to be sent by the QUIC Thread.
    */
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8]
    ) -> Poll<Result<usize, std::io::Error>> {
        //Write
        //let data = self.receiverCS_ref.lock().unwrap().recv().unwrap();
        //println!("Received data: {:?}", data);
        //buf.put_slice(&data);
        println!("Polling the write");
        println!("Wants to write {:?}", buf);
        self.senderSC.send(buf.to_vec()).unwrap();
        Poll::Ready(Ok(buf.len()))
    }

    /*
        Attempts to flush the object, ensuring that any buffered data reach their destination.
     */
    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>
    ) -> Poll<Result<(), std::io::Error>> {
        // Not needed
        println!("Polling the flush");
        //Poll::Pending
        Poll::Ready(Ok(()))
    }


    /*       
       Initiates or attempts to shut down this writer, returning success when the I/O connection has completely shut down.
     */
    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>
    ) -> Poll<Result<(), std::io::Error>> {
        // Close the gracefully the connection
        //
        println!("Polling the shutdown");   
        //Poll::Pending
        Poll::Ready(Ok(()))
    }
}

impl Connected for IO {
    type ConnectInfo = MyConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        MyConnectInfo {}
    }
}

#[derive(Clone)]
pub struct MyConnectInfo {
    // Metadata about the connection
}

// --------------- Functions coming from quiche example --> Not currently used. -------------------


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

/// Handles incoming HTTP/3 requests.
fn handle_request(
    client: &mut Client, stream_id: u64, headers: &[quiche::h3::Header], 
    root: &str,receiverSC: flume::Receiver<Vec<u8>>,
) {
    let conn = &mut client.conn;
    let http3_conn = &mut client.http3_conn.as_mut().unwrap();

    println!(
        "{} got request {:?} on stream id {}",
        conn.trace_id(),
        hdrs_to_strings(headers),
        stream_id
    );

    // We decide the response based on headers alone, so stop reading the
    // request stream so that any body is ignored and pointless Data events
    // are not generated.
    /* 
    conn.stream_shutdown(stream_id, quiche::Shutdown::Read, 0)
        .unwrap();
    */

    let (headers, body) = build_response(root, headers, receiverSC.clone());

    match http3_conn.send_response(conn, stream_id, &headers, false) {
        Ok(v) => v,

        Err(quiche::h3::Error::StreamBlocked) => {
            let response = PartialResponse {
                headers: Some(headers),
                body,
                written: 0,
            };

            client.partial_responses.insert(stream_id, response);
            return;
        },

        Err(e) => {
            error!("{} stream send failed {:?}", conn.trace_id(), e);
            return;
        },
    }

    let written = match http3_conn.send_body(conn, stream_id, &body, true) {
        Ok(v) => v,

        Err(quiche::h3::Error::Done) => 0,

        Err(e) => {
            error!("{} stream send failed {:?}", conn.trace_id(), e);
            return;
        },
    };

    if written < body.len() {
        let response = PartialResponse {
            headers: None,
            body,
            written,
        };

        client.partial_responses.insert(stream_id, response);
    }
}

/// Builds an HTTP/3 response given a request.
fn build_response(
    root: &str, request: &[quiche::h3::Header], receiverSC: flume::Receiver<Vec<u8>>
) -> (Vec<quiche::h3::Header>, Vec<u8>) {
    let mut file_path = std::path::PathBuf::from(root);
    let mut path = std::path::Path::new("");
    let mut method = None;

    // Look for the request's path and method.
    for hdr in request {
        match hdr.name() {
            b":path" =>
                path = std::path::Path::new(
                    std::str::from_utf8(hdr.value()).unwrap(),
                ),

            b":method" => method = Some(hdr.value()),

            _ => (),
        }
    }

    let (status, body) = match method {
        Some(b"GET") => {
            /*  
            for c in path.components() {
                if let std::path::Component::Normal(v) = c {
                    file_path.push(v)
                }
            }

            match std::fs::read(file_path.as_path()) {
                Ok(data) => (200, data),

                Err(_) => (404, b"Not Found!".to_vec()),
            } */
            match receiverSC.recv() {
                Ok(msg) => {println!("responding to {:?}", msg); 
                        (200, msg)
                    }

                Err(_) => {println!("breaking this loop"); (404, b"Not Found!".to_vec())}
            }
        },

        _ => (405, Vec::new()),
    };

    let headers = vec![
        quiche::h3::Header::new(b":status", status.to_string().as_bytes()),
        quiche::h3::Header::new(b"server", b"quiche"),
        quiche::h3::Header::new(
            b"content-length",
            body.len().to_string().as_bytes(),
        ),
    ];

    (headers, body)
}

/// Handles newly writable streams.
fn handle_writable(client: &mut Client, stream_id: u64) {
    let conn = &mut client.conn;
    let http3_conn = &mut client.http3_conn.as_mut().unwrap();

    debug!("{} stream {} is writable", conn.trace_id(), stream_id);

    if !client.partial_responses.contains_key(&stream_id) {
        return;
    }

    let resp = client.partial_responses.get_mut(&stream_id).unwrap();

    if let Some(ref headers) = resp.headers {
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

pub fn hdrs_to_strings(hdrs: &[quiche::h3::Header]) -> Vec<(String, String)> {
    hdrs.iter()
        .map(|h| {
            let name = String::from_utf8_lossy(h.name()).to_string();
            let value = String::from_utf8_lossy(h.value()).to_string();

            (name, value)
        })
        .collect()
}