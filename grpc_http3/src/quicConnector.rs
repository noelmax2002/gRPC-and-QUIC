//use tonic::transport::{Channel};
use tower::Service;
use std::task::{Context, Poll};
use std::pin::Pin;
use tonic::transport::Uri;
use hyper::body::Bytes;
use hyper::body::Frame;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::io;
use hyper::rt::{Write,Read}:
use quiche::h3::Config;
use quiche::h3::Connection;


pub struct QuicConnector {
    //define attributes for the connector
    conn: Connection,
}

impl QuicConnector {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
       //initialize the connection
       let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
       config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;

       let server_addr: SocketAddr = "0.0.0.1:50051".parse()?; //which address to use here?
       let local: SocketAddr = "0.0.0.1:50051".parse()?; //which address to use here?
       let scid = ConnectionId::from_ref(&[0xba, 0xad, 0xf0, 0x0d]);
       let conn = quiche::connect(None, &scid, local, server_addr, &mut config)?;
       let h3_config = quiche::h3::Config::new()?;
       let h3_conn = quiche::h3::Connection::with_transport(&mut conn, &h3_config)?;

        Ok(QuicConnector {h3_conn,})
    }
}

impl Service<Uri> for QuicConnector {
    type Response = Connection;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        //determines if the connector is ready.
        //no specific conditions here so always ready.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        // return a futur when the connection is ready
        future::ready(Ok(Connection::new(self.clone())))
    }
}

pub struct Connection {
    connector: QuicConnector,
}

impl Connection {
    fn new(connector: QuicConnector) -> Self {
        Connection { connector }
    }
}

impl hyper::body::Body for Connection {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // Implement based on your needs
        Poll::Ready(None)  // Adjust this based on actual QUIC data handling
    }
} 

impl Read for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // READ PACKETS
        Poll::Pending
    }
}

impl Write for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // WRITTE PACKETS
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Not needed
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Close the gracefully the connection
        Poll::Ready(Ok(()))
    }
}

