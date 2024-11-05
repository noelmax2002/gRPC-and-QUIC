use std::task::{Context, Poll};
use std::pin::Pin;
use std::net::SocketAddr;
use quiche::ConnectionId;
use futures_core::stream::Stream;
use tokio::io::{AsyncRead, AsyncWrite};
use tonic::transport::server::Connected;
use std::io::Error;

pub struct QuicStream {
    conn: quiche::Connection,
}


impl QuicStream {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
       //initialize the connection
       
       let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
       config.set_application_protos(quiche::h3::APPLICATION_PROTOCOL)?;
       let server_addr: SocketAddr = "0.0.0.2:50051".parse()?; //which address to use here?
       let local: SocketAddr = "0.0.0.1:50051".parse()?; //which address to use here?
       let scid = ConnectionId::from_ref(&[0xba, 0xad, 0xf0, 0x0d]);
       let conn = quiche::connect(None, &scid, local, server_addr, &mut config).expect("Error in the quic connection");
        
        Ok(QuicStream {conn: conn})
    }
}

impl Stream for QuicStream{
    type Item = Result<IO, Error>;

    // Required method
    fn poll_next(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>
    ) -> Poll<Option<Self::Item>>{
        // Poll the connection
        let h3_conf = quiche::h3::Config::new().expect("Cannot load the h3 config");
        let connection = quiche::h3::Connection::with_transport(&mut self.conn,&h3_conf).expect("Failed to create HTTP/3 connection");
        Poll::Ready(Some(Ok(IO::new(connection))))
    }

}

pub struct IO {
    //define attributes for the IO
    conn: quiche::h3::Connection,
}

impl IO {
    pub fn new(conn: quiche::h3::Connection) -> Self {
        IO {
            conn: conn,
        }
    }
}

impl AsyncRead for IO {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut tokio::io::ReadBuf
    ) -> Poll<Result<(), std::io::Error>> {
        // READ PACKETS
        Poll::Pending
    }
}

impl AsyncWrite for IO {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8]
    ) -> Poll<Result<usize, std::io::Error>> {
        //Write
        Poll::Pending
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>
    ) -> Poll<Result<(), std::io::Error>> {
        // Not needed
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>
    ) -> Poll<Result<(), std::io::Error>> {
        // Close the gracefully the connection
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
    // Metadata about your connection
}
