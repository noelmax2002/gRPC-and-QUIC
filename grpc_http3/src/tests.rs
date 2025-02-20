mod client;
mod server;

use crate::hello_world::HelloReply;
use crate::client::run_client;

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

pub mod hello_world {
    tonic::include_proto!("helloworld");
}


#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    /*
        Simple test to check if the server and client can communicate with each other.
     */
    async fn hello_world_test() -> Result<(), Box<dyn std::error::Error>> {

        let (sender, receiver) = tokio::sync::oneshot::channel(); //Channel to let the thread know that the server must be shut down.
        let join_handle: task::JoinHandle<()> = task::spawn(async {
            let mut child = if cfg!(target_os = "windows") {
                Command::new("cargo")
                    .args(["run", "--bin", "server"])
                    .spawn()
                    .expect("failed to execute the server")
            } else {
                Command::new("cargo")
                    .args(["run", "--bin", "server"])
                    .spawn()
                    .expect("failed to execute the server")
            };

            receiver.await.unwrap();
            child.kill().expect("failed to kill server");
        });

        
        sleep(Duration::from_secs(2)).await; // Let the server be up

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
        
        println!("Response: {:?}", response);
        assert!(true);
        assert_eq!(response.get_ref().message, "Hello Maxime!");
        sender.send(1).unwrap();
        //join_handle.abort();
        

        println!("Test Passed");
    
        Ok(())
    }
}