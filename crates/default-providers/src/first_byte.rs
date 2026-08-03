//! A deadline on the first byte of a connection, and on nothing after it.
//!
//! Every other timeout an HTTP server has starts once hyper owns the
//! connection — but hyper does not own it yet while it is reading the bytes
//! that say *which version* this is (hyper-util's version detection peeks up to
//! 24 bytes, the length of the HTTP/2 preface, before it can build either kind
//! of connection). A peer that completes the TCP handshake and then says
//! nothing sits in that gap, holding a task and a descriptor, and no timer is
//! running: `header_read_timeout` belongs to a connection that has not been
//! constructed.
//!
//! [`FirstByteTimeout`] closes the gap from underneath. It wraps the stream, so
//! the deadline applies wherever those first bytes are read from, and it stops
//! meaning anything the moment one byte arrives — after that every read and
//! write is a straight delegation, because from then on the timeouts that hyper
//! runs are the right ones and a long-lived connection must not be interrupted
//! by this.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep};

/// Wraps a stream with a deadline that expires unless a first byte is read.
///
/// Once any byte has been read the wrapper is inert: it holds no timer and adds
/// a single branch per read. Constructed with `None`, it is inert from the
/// start.
pub(crate) struct FirstByteTimeout<S> {
    inner: S,
    /// The pending deadline, dropped once the first byte lands.
    deadline: Option<Pin<Box<Sleep>>>,
}

impl<S> FirstByteTimeout<S> {
    /// Wraps `inner`, failing the connection if no byte is read within
    /// `within`. `None` disables the deadline.
    pub(crate) fn new(inner: S, within: Option<Duration>) -> Self {
        Self {
            inner,
            deadline: within.map(|d| Box::pin(tokio::time::sleep_until(Instant::now() + d))),
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for FirstByteTimeout<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let Some(deadline) = this.deadline.as_mut() else {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        };

        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Pending => {
                // Only now does the deadline matter: the peer has sent nothing
                // this wrapper could hand on.
                ready!(deadline.as_mut().poll(cx));
                this.deadline = None;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for the first byte of the connection",
                )))
            }
            // Bytes, EOF, or an error: whichever it is, the wait is over and
            // there is nothing left for the deadline to protect against.
            ready => {
                this.deadline = None;
                ready
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FirstByteTimeout<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    const SHORT: Duration = Duration::from_millis(100);

    #[tokio::test]
    async fn a_stream_that_never_speaks_times_out() {
        let (a, _b) = duplex(64); // `_b` held open, and silent
        let mut wrapped = FirstByteTimeout::new(a, Some(SHORT));

        let err = wrapped.read_u8().await.expect_err("must not wait forever");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn one_byte_disarms_the_deadline_for_good() {
        let (a, mut b) = duplex(64);
        let mut wrapped = FirstByteTimeout::new(a, Some(SHORT));

        b.write_all(b"h").await.unwrap();
        assert_eq!(wrapped.read_u8().await.unwrap(), b'h');

        // Well past the original deadline, a second read must still wait for
        // the peer rather than fail — this is the long-lived-connection case,
        // and it is the whole reason the wrapper disarms.
        tokio::time::sleep(SHORT * 3).await;
        let read = tokio::spawn(async move { wrapped.read_u8().await });
        tokio::time::sleep(SHORT * 2).await;
        assert!(!read.is_finished(), "a disarmed wrapper never times out");
        b.write_all(b"i").await.unwrap();
        assert_eq!(read.await.unwrap().unwrap(), b'i');
    }

    #[tokio::test]
    async fn no_deadline_means_no_deadline() {
        let (a, mut b) = duplex(64);
        let mut wrapped = FirstByteTimeout::new(a, None);

        let read = tokio::spawn(async move { wrapped.read_u8().await });
        tokio::time::sleep(SHORT * 3).await;
        assert!(!read.is_finished(), "an unset deadline never fires");
        b.write_all(b"h").await.unwrap();
        assert_eq!(read.await.unwrap().unwrap(), b'h');
    }

    #[tokio::test]
    async fn writes_pass_straight_through() {
        let (a, mut b) = duplex(64);
        let mut wrapped = FirstByteTimeout::new(a, Some(SHORT));

        wrapped.write_all(b"out").await.unwrap();
        wrapped.flush().await.unwrap();
        let mut got = [0u8; 3];
        b.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"out");
    }
}
