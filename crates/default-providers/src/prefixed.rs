//! A stream with bytes in front of it: what was read from a connection before
//! the code that will speak its protocol took it over.
//!
//! The server reads a connection's first bytes to choose HTTP/1.1 or HTTP/2,
//! and an HTTP/1.1 connection that upgrades has read past its head; in both
//! cases those bytes belong to whatever reads next, and this hands them over
//! first.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// `inner`, read as `prefix` followed by whatever `inner` yields.
pub(crate) struct Prefixed<S> {
    prefix: Vec<u8>,
    at: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub(crate) fn new(inner: S, prefix: Vec<u8>) -> Self {
        Prefixed {
            prefix,
            at: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.at < this.prefix.len() {
            let n = (this.prefix.len() - this.at).min(out.remaining());
            out.put_slice(&this.prefix[this.at..this.at + n]);
            this.at += n;
            if this.at == this.prefix.len() {
                this.prefix = Vec::new();
                this.at = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, out)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
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
    use super::Prefixed;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn the_prefix_is_read_first_then_the_stream() {
        let inner: &[u8] = b" world";
        let mut s = Prefixed::new(inner, b"hello".to_vec());
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn a_small_read_takes_the_prefix_in_pieces() {
        let inner: &[u8] = b"!";
        let mut s = Prefixed::new(inner, b"abc".to_vec());
        let mut two = [0u8; 2];
        assert_eq!(s.read(&mut two).await.unwrap(), 2);
        assert_eq!(&two, b"ab");
        let mut rest = Vec::new();
        s.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"c!");
    }
}
