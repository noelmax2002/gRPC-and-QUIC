use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use echo::{EchoRequest, EchoResponse};
use filetransfer::file_service_server::{FileService, FileServiceServer};
use filetransfer::{FileData, FileRequest, UploadResponse};

use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tonic::Streaming;
use std::error::Error;
use tokio::time::Duration;
use std::pin::Pin;
use docopt::Docopt;
use tokio::io::ErrorKind;
use std::net::SocketAddr;



#[cfg(feature = "tls")]
use tonic::{
    transport::{
        Identity, Server, ServerTlsConfig,
    },
    Request, Response, Status,
};


pub mod hello_world {
    tonic::include_proto!("helloworld");
}

pub mod echo {
    tonic::include_proto!("echo");
}

pub mod filetransfer {
    tonic::include_proto!("filetransfer");
}


const USAGE: &'static str = "
Usage: server [options]

Options:
    -s --sip ADDRESS ...       Server IPv4 address and port [default: 127.0.0.1:4433].
    --server-pem PATH       Path to server.pem [default: ./HTTP2/tls/server.pem].
    --server-key PATH       Path to server.key [default: ./HTTP2/tls/server.key].
    -p --proto PROTOCOL     ProtoBuf to use [default: helloworld].
";

#[derive(Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        println!("Got a request from {:?}", request.remote_addr());

        let reply = hello_world::HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        Ok(Response::new(reply))
    }
}
//[::1]:50051
//192.168.1.7:8080

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {


    let args = Docopt::new(USAGE).expect("Problem during the parsing").parse().unwrap_or_else(|e| e.exit());
    let proto = args.get_str("--proto").to_string();
    
    let mut server_pem = args.get_str("--server-pem").to_string();
    let mut server_key = args.get_str("--server-key").to_string();

    let mut server_addrs: Vec<SocketAddr> = args
        .get_vec("--sip")
        .into_iter()
        .map(|addr| addr.parse().expect("Invalid IP address"))
        .collect();

    
    let addr = server_addrs[0];
    

    if cfg!(target_os = "windows") {
        server_pem = "./src/HTTP2/tls/server.pem".to_string();
        server_key = "./src/HTTP2/tls/server.key".to_string();
    }
    let cert = std::fs::read_to_string(server_pem).expect("Failed to read server.pem");
    let key = std::fs::read_to_string(server_key).expect("Failed to read server.key");
    let identity = Identity::from_pem(cert, key);

    if server_addrs.len() == 1 {

        // Create tonic server builder.
        if proto == "helloworld" {
            let greeter = MyGreeter::default(); //Define the gRPC service.
            Server::builder()
            .tls_config(ServerTlsConfig::new().identity(identity))?
            .add_service(GreeterServer::new(greeter))
            .serve(addr)
            .await?;
            println!("GreeterServer listening on {}", addr);
        } else if proto == "echo" {
            let echo = EchoServer {};
            Server::builder()
            .tls_config(ServerTlsConfig::new().identity(identity))?
            .add_service(echo::echo_server::EchoServer::new(echo))
            .serve(addr)
            .await?;
            println!("EchoServer listening on {}", addr);

        } else if proto == "filetransfer" || proto == "fileexchange" {
            let file_service = MyFileService::default();
            Server::builder()
            .tls_config(ServerTlsConfig::new().identity(identity))?
            .add_service(FileServiceServer::new(file_service))
            .serve(addr)
            .await?;
            println!("FileTransferServer listening on {}", addr);
        } else {
            panic!("Unknown protocol: {:?}", proto);
        }
    } else {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for addr in &server_addrs {
            let serve;
            let tx = tx.clone();
            if proto == "helloworld" {
                let greeter = MyGreeter::default(); //Define the gRPC service.
                serve = Server::builder()
                .tls_config(ServerTlsConfig::new().identity(identity.clone()))?
                .add_service(GreeterServer::new(greeter))
                .serve(*addr);
                println!("GreeterServer listening on {}", addr);
            } else if proto == "echo" {
                let echo = EchoServer {};
                serve = Server::builder()
                .tls_config(ServerTlsConfig::new().identity(identity.clone()))?
                .add_service(echo::echo_server::EchoServer::new(echo))
                .serve(*addr);
                println!("EchoServer listening on {}", addr);

            } else if proto == "filetransfer" || proto == "fileexchange" {
                let file_service = MyFileService::default();
                serve = Server::builder()
                .tls_config(ServerTlsConfig::new().identity(identity.clone()))?
                .add_service(FileServiceServer::new(file_service))
                .serve(*addr);
                println!("FileTransferServer listening on {}", addr);
            } else {
                panic!("Unknown protocol: {:?}", proto);
            }

            tokio::spawn(async move {
                if let Err(e) = serve.await {
                    eprintln!("Error = {:?}", e);
                }
    
                tx.send(()).unwrap();
            });
        }   
        rx.recv().await;
    }

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
