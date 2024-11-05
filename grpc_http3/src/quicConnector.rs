//use tonic::transport::{Channel};
use tower_service;
use std::task::{Context, Poll};
use std::pin::Pin;
use tonic::transport::Uri;
use hyper::rt::{Write,Read};
use std::net::SocketAddr;
use quiche::ConnectionId;
use hyper::rt::ReadBufCursor;
use std::io::Error;
use futures_util::future::{self,Ready};

pub struct QuicConnector {
    //define attributes for the connector
    conn: quiche::Connection,
    h3_config: quiche::h3::Config,
}


impl QuicConnector {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
       //initialize the connection
       let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
       config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
       let server_addr: SocketAddr = "0.0.0.1:50051".parse()?; //which address to use here?
       let local: SocketAddr = "0.0.0.2:50051".parse()?; //which address to use here?
       let scid = ConnectionId::from_ref(&[0xba, 0xad, 0xf0, 0x0d]);
       let conn = quiche::connect(None, &scid, local, server_addr, &mut config).expect("Error in the quic connection");
       let h3_config = quiche::h3::Config::new().expect("Cannot load the h3 config");

        Ok(QuicConnector {conn: conn, h3_config: h3_config})
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
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        // return a futur when the connection is ready
        future::ready(Ok(HTTP3Connection::new(quiche::h3::Connection::with_transport(&mut self.conn, &self.h3_config).expect("Failed to create HTTP/3 connection"))))
    }
}

pub struct HTTP3Connection {
    connector: quiche::h3::Connection,
}

impl HTTP3Connection {
    pub fn new(connector: quiche::h3::Connection) -> Self {
        HTTP3Connection {
            connector: connector,
        }
    }
}


impl Read for HTTP3Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: ReadBufCursor<'_>,
    ) -> Poll<Result<(), Error>> {
        // READ PACKETS
        Poll::Pending
    }
}

impl Write for HTTP3Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        // WRITTE PACKETS
        Poll::Pending
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

