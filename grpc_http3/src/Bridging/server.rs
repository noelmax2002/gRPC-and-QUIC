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
use std::io::Write;

use tokio::io::Interest;
use ring::rand::*;
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::net::TcpListener;
use tokio::net::TcpStream;


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
const MAX_BRIDGING_BUFFER_SIZE: usize = 10000;
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
        
        // Communication with gRPC.
        let (to_client, from_client) = tokio::io::duplex(1_000_000);

        // Communication with IO.
        let (txx, mut rxx) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        //println!("Creating new gRPC channel with id {:?}", client.id);

        let mut new_client = Client {
            stream: to_client,
            to_io: txx,
            from_io: rx2,
            id: 1,
        };


        // Let the client run on its own.
        tokio::spawn(async move {
            new_client.run().await.unwrap();
        });
        tokio::task::yield_now().await;

        // Notify gRPC of the new client.
        self.to_grpc.send(Ok(from_client))?;
        
        
        let listener = TcpListener::bind("127.0.0.1:4545")?;

        let stream = match listener.incoming().next() {
            Some(Ok(stream)) => stream,
            Some(Err(e)) => {
                panic!("Failed to accept connection: {}", e);
            }
            None => {
                panic!("No incoming connection found");
            }
        };

        println!("Accepted connection from {:?}", stream.peer_addr());
        let mut TStream = TcpStream::from_std(stream)?;
        TStream.set_nodelay(true)?;

    
        let mut buffer = [0u8; 100000];
        loop {
            tokio::select! {
                result = TStream.read(&mut buffer) => {
                    let len = result?;
                    if len == 0 {
                        println!("Connection closed");
                        return Ok(());
                    }
                    tx2.send((&buffer[..len]).to_vec()).ok();
                },


                Some(data) = rxx.recv() => {
                    TStream.write_all(&data).await?;
                },
            }
        }
                  


        
        

        Ok(())
   
    }
    
}

struct Client {
    stream: DuplexStream,
    to_io: UnboundedSender<Vec<u8>>,
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
        self.to_io.send(msg.to_vec());
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

    let proto = "fileexchange";
    
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

