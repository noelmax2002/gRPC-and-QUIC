mod client;
mod server;

use crate::hello_world::HelloReply;
use crate::client::run_client;

use echo::{echo_client::EchoClient, EchoRequest};
use filetransfer::file_service_client::FileServiceClient;
use filetransfer::{FileData, FileRequest};

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;
use tonic::transport::{Endpoint,Uri};
use tower::service_fn;
use hyper_util::rt::tokio::TokioIo;
use tokio::sync::mpsc;
use tokio::task;
use tokio::time::Duration;
use tokio::time::sleep;
use std::time::{SystemTime, UNIX_EPOCH};
use std::process::Command;
use tonic::Response;
use futures_util::future;
use futures_util::TryFutureExt;
use tokio_stream::{Stream, StreamExt};
use tokio::fs::{self,File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

pub mod echo {
    tonic::include_proto!("echo");
}

pub mod filetransfer {
    tonic::include_proto!("filetransfer");
}


#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    
    #[tokio::test(flavor = "multi_thread")]
    async fn tests(){
        hello_world_test().await.unwrap();
        sleep(Duration::from_secs(1)).await;
        echo_bidirectionnal_test().await.unwrap();
        sleep(Duration::from_secs(1)).await;
        multiples_hello_world_test().await.unwrap();
        sleep(Duration::from_secs(1)).await;
        file_upload_test("./swift_file_examples/small.txt".to_string()).await.unwrap();
        sleep(Duration::from_secs(1)).await;
        file_download_test("./swift_file_examples/small.txt".to_string()).await.unwrap();
        sleep(Duration::from_secs(1)).await;
        
        file_upload_test("./swift_file_examples/big.txt".to_string()).await.unwrap();
        sleep(Duration::from_secs(1)).await;
        /*
        file_download_test("./swift_file_examples/big.txt".to_string()).await.unwrap();
        sleep(Duration::from_secs(1)).await;  
        */
        
    }


    /*
        Simple test to check if the server and client can communicate with each other.
     */
    async fn hello_world_test() -> Result<(), Box<dyn std::error::Error>> {

        let (sender, receiver) = tokio::sync::oneshot::channel(); //Channel to let the thread know that the server must be shut down.
        task::spawn(async {
            let mut child = if cfg!(target_os = "windows") {
                Command::new("cargo")
                    .args(["run", "--bin", "server"])
                    .spawn()
                    .expect("failed to execute the server")
            } else {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "--cert-file", "./src/cert.crt", "--key-file", "./src/cert.key"])
                    .spawn()
                    .expect("failed to execute the server")
            };

            receiver.await.unwrap();
            child.kill().expect("failed to kill server");
        });

        
        sleep(Duration::from_secs(10)).await; // Let the server be up

        let channel = Endpoint::from_static("https://127.0.0.1:4433")
            .connect_with_connector(service_fn(|uri: Uri| async {
                let (client, server) = tokio::io::duplex(12000);
                task::spawn(async move {
                    let (to_client, from_io) = mpsc::channel(1000);
                    let (to_io, from_client) = mpsc::channel(1000);
    
                    let mut new_client = client::Client {
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
        })).await.unwrap();
    
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        let mut client = GreeterClient::new(channel);
    
        let request = tonic::Request::new(HelloRequest {
            name: "Maxime".into(),
        });
        
        
        let response = client.say_hello(request).await.unwrap();

        let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        println!("Time elapsed: {:?}", end - start);
        
        assert_eq!(response.get_ref().message, "Hello Maxime!");
        sender.send(1).unwrap();
        
        Ok(())
    }

    /*
        Simple test to check if the server and client can send multiple messages to each other (not in a stream).
     */
    async fn multiples_hello_world_test() -> Result<(), Box<dyn std::error::Error>> {

        let (sender, receiver) = tokio::sync::oneshot::channel(); //Channel to let the thread know that the server must be shut down.
        task::spawn(async {
            let mut child = if cfg!(target_os = "windows") {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-s", "127.0.0.1:4400", "--timeout", "50000"])
                    .spawn()
                    .expect("failed to execute the server")
            } else {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-s", "127.0.0.1:4400", "--timeout", "50000", "--cert-file", "./src/cert.crt", "--key-file", "./src/cert.key"])
                    .spawn()
                    .expect("failed to execute the server")
            };
        
            receiver.await.unwrap();
            child.kill().expect("failed to kill server");
        });

        
        sleep(Duration::from_secs(10)).await; // Let the server be up

        let channel = Endpoint::from_static("https://127.0.0.1:4400")
            .connect_with_connector(service_fn(|uri: Uri| async {
                let (client, server) = tokio::io::duplex(12000);
                task::spawn(async move {
                    let (to_client, from_io) = mpsc::channel(1000);
                    let (to_io, from_client) = mpsc::channel(1000);
    
                    let mut new_client = client::Client {
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
        })).await.unwrap();
    
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        let mut client = GreeterClient::new(channel);
    
        let request = tonic::Request::new(HelloRequest {
            name: "Maxime".into(),
        });

        let request2 = tonic::Request::new(HelloRequest {
            name: "Guillaume".into(),
        });

        let request3 = tonic::Request::new(HelloRequest {
            name: "Elisa".into(),
        });

        let request4 = tonic::Request::new(HelloRequest {
            name: "Théo".into(),
        });

        let request5 = tonic::Request::new(HelloRequest {
            name: "Maelle".into(),
        });
        
    
        let response = client.say_hello(request).await.unwrap();
        assert_eq!(response.get_ref().message, "Hello Maxime!");
        println!("response: {:?}", response.get_ref().message);

        let response2 = client.say_hello(request2).await.unwrap();
        assert_eq!(response2.get_ref().message, "Hello Guillaume!");
        println!("response: {:?}", response2.get_ref().message);


        let response3 = client.say_hello(request3).await.unwrap();
        assert_eq!(response3.get_ref().message, "Hello Elisa!");
        println!("response: {:?}", response3.get_ref().message);


        let response4 = client.say_hello(request4).await.unwrap();
        assert_eq!(response4.get_ref().message, "Hello Théo!");
        println!("response: {:?}", response4.get_ref().message);


        let response5 = client.say_hello(request5).await.unwrap();
        assert_eq!(response5.get_ref().message, "Hello Maelle!");
        println!("response: {:?}", response5.get_ref().message);


        let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        println!("Time elapsed: {:?}", end - start);
        
        sender.send(1).unwrap();
        
        Ok(())
    }
 

    /*
        Simple test to check bidirectionnal streaming between the server and the client.
     */
    async fn echo_bidirectionnal_test() -> Result<(), Box<dyn std::error::Error>> {

        let (sender, receiver) = tokio::sync::oneshot::channel(); //Channel to let the thread know that the server must be shut down.
        task::spawn(async {
            let mut child = if cfg!(target_os = "windows") {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-s", "127.0.0.1:3344","-p", "echo"])
                    .spawn()
                    .expect("failed to execute the server")
            } else {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-s", "127.0.0.1:3344","-p", "echo","--cert-file", "./src/cert.crt", "--key-file", "./src/cert.key"])
                    .spawn()
                    .expect("failed to execute the server")
            };
    
            receiver.await.unwrap();
            child.kill().expect("failed to kill server");
        });

        
        sleep(Duration::from_secs(10)).await; // Let the server be up

        let channel = Endpoint::from_static("https://127.0.0.1:3344")
            .connect_with_connector(service_fn(|uri: Uri| async {
                let (client, server) = tokio::io::duplex(12000);
                task::spawn(async move {
                    let (to_client, from_io) = mpsc::channel(1000);
                    let (to_io, from_client) = mpsc::channel(1000);
    
                    let mut new_client = client::Client {
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
        })).await.unwrap();
    
        let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        let mut client = EchoClient::new(channel);
        let num = 50;

        let in_stream = echo_requests_iter().take(num);

        let response = client
            .bidirectional_streaming_echo(in_stream)
            .await
            .unwrap();
    
        let mut resp_stream = response.into_inner();
    
        let mut i = 1;

        while let Some(received) = resp_stream.next().await {
            let received = received.unwrap();
            println!("\treceived message: `{}`", received.message);
            assert_eq!(received.message, format!("msg {:02}", i));
            i += 1;
        }    
        assert_eq!(i, num + 1); //all the stream has been sent

        let end = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        println!("Time elapsed: {:?}", end - start);
        
        sender.send(1).unwrap();
        
        Ok(())

    }

    fn echo_requests_iter() -> impl Stream<Item = EchoRequest> {
        tokio_stream::iter(1..usize::MAX).map(|i| EchoRequest {
            message: format!("msg {:02}", i),
        })
    }

    /*
        Simple test to check if the server and the client can send a file to each other.
     */
    async fn file_upload_test(name: String) -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = tokio::sync::oneshot::channel(); //Channel to let the thread know that the server must be shut down.
        task::spawn(async {
            let mut child = if cfg!(target_os = "windows") {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-p", "filetransfer"])
                    .spawn()
                    .expect("failed to execute the server")
            } else {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-p", "filetransfer", "--cert-file", "./src/cert.crt", "--key-file", "./src/cert.key"])
                    .spawn()
                    .expect("failed to execute the server")
            };
    
            receiver.await.unwrap();
            child.kill().expect("failed to kill server");
        });

        sleep(Duration::from_secs(10)).await; // Let the server be up

        let channel = Endpoint::from_static("https://127.0.0.1:4433")
            .connect_with_connector(service_fn(|uri: Uri| async {
                let (client, server) = tokio::io::duplex(12000);
                task::spawn(async move {
                    let (to_client, from_io) = mpsc::channel(1000);
                    let (to_io, from_client) = mpsc::channel(1000);
    
                    let mut new_client = client::Client {
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
        })).await.unwrap();

        let mut client = FileServiceClient::new(channel);

        // Upload a file
        let file_path = name;
        let mut file = File::open(file_path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        let request = tonic::Request::new(FileData {
            filename: "uploaded_example".into(),
            data: buffer.clone(),
        });

        let response = client.upload_file(request).await?.into_inner();
        println!("Upload Response: {}", response.message);
        assert_eq!(response.message, "File uploaded successfully");

        let file_uploaded_path = "./uploaded_example";
        let mut file_uploaded = File::open(file_uploaded_path).await?;
        let mut buffer_uploaded = Vec::new();
        file_uploaded.read_to_end(&mut buffer_uploaded).await?;

        assert_eq!(buffer, buffer_uploaded);

        sender.send(1).unwrap();

        Ok(())
    }

    /*
        Simple test to check if the server and the client can send a file to each other.
     */
    async fn file_download_test(name: String,) -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = tokio::sync::oneshot::channel(); //Channel to let the thread know that the server must be shut down.
        task::spawn(async {
            let mut child = if cfg!(target_os = "windows") {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-p", "filetransfer"])
                    .spawn()
                    .expect("failed to execute the server")
            } else {
                Command::new("cargo")
                    .args(["run", "--bin", "server", "--", "-p", "filetransfer","--cert-file", "./src/cert.crt", "--key-file", "./src/cert.key"])
                    .spawn()
                    .expect("failed to execute the server")
            };
           
            receiver.await.unwrap();
            child.kill().expect("failed to kill server");
        });

        sleep(Duration::from_secs(10)).await; // Let the server be up

        let channel = Endpoint::from_static("https://127.0.0.1:4433")
            .connect_with_connector(service_fn(|uri: Uri| async {
                let (client, server) = tokio::io::duplex(12000);
                task::spawn(async move {
                    let (to_client, from_io) = mpsc::channel(1000);
                    let (to_io, from_client) = mpsc::channel(1000);
    
                    let mut new_client = client::Client {
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
        })).await.unwrap();

        let mut client = FileServiceClient::new(channel);
        
        // Download a file
        let request = tonic::Request::new(FileRequest {
            filename: "uploaded_example".into(),
        });

        let response = client.download_file(request).await?.into_inner();
        let mut new_file = File::create("downloaded_example").await?;
        new_file.write_all(&response.data).await?;

        println!("File downloaded successfully!");

        let file_path = name;
        let mut file = File::open(file_path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        assert_eq!(buffer, response.data);

        //remove the downloaded and uploaded file
        fs::remove_file("./uploaded_example").await?;
        fs::remove_file("./downloaded_example").await?;


        sender.send(1).unwrap();

        Ok(())
    }

        
}