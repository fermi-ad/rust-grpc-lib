//! Force-close accepted sockets without relying on tonic's detached tasks
//! being dropped with the serve future.

use std::future::Future;
use std::io::{self, IoSlice};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Notify, futures::OwnedNotified};
use tonic::transport::server::{Connected, TcpConnectInfo, TcpIncoming};

#[derive(Default)]
pub(super) struct Connections(Arc<Notify>);

impl Connections {
    pub(super) fn incoming(
        &self,
        incoming: TcpIncoming,
    ) -> impl Stream<Item = io::Result<Socket>> + use<> {
        let close = self.0.clone();
        incoming.with_nodelay(Some(true)).map(move |result| {
            result.map(|stream| Socket {
                stream: Some(stream),
                closed: Box::pin(close.clone().notified_owned()),
            })
        })
    }

    // Only called once the accept loop has stopped (or is being dropped).
    pub(super) fn close(&self) {
        // Unlike notify_one, this reaches every socket, even one whose
        // notification future has been created but not yet polled.
        self.0.notify_waiters();
    }
}

impl Drop for Connections {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) struct Socket {
    stream: Option<TcpStream>,
    closed: Pin<Box<OwnedNotified>>,
}

impl Socket {
    fn live(&mut self, cx: &mut Context<'_>) -> io::Result<Pin<&mut TcpStream>> {
        // Register the connection task's waker even when the underlying socket
        // has no pending readiness. Expiry must wake idle HTTP/2 connections.
        if self.closed.as_mut().poll(cx).is_ready() {
            drop(self.stream.take());
        }
        self.stream
            .as_mut()
            .map(Pin::new)
            .ok_or_else(|| io::ErrorKind::ConnectionAborted.into())
    }
}

impl Connected for Socket {
    type ConnectInfo = TcpConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        // tonic calls this on accept, before polling I/O.
        self.stream.as_ref().unwrap().connect_info()
    }
}

impl AsyncRead for Socket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.live(cx)?.poll_read(cx, buf)
    }
}

impl AsyncWrite for Socket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.live(cx)?.poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.live(cx)?.poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream
            .as_ref()
            .is_some_and(AsyncWrite::is_write_vectored)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.live(cx)?.poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.live(cx)?.poll_shutdown(cx)
    }
}
