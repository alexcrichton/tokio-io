use {AsyncRead, AsyncWrite};
use framed_read::{framed_read2, FramedRead2, Decoder};
use framed_write::{framed_write2, FramedWrite2, Encoder};

use futures::{Stream, Sink, StartSend, Poll};
use bytes::{BytesMut};

use std::io::{self, Read, Write};

/// A unified `Stream` and `Sink` interface to an underlying I/O object, using
/// the `Encoder` and `Decoder` traits to encode and decode frames.
///
/// You can create a `Framed` instance by using the `AsyncRead::framed` adapter.
pub struct Framed<T, U> {
    inner: FramedRead2<FramedWrite2<Fuse<T, U>>>,
}

pub struct Fuse<T, U>(pub T, pub U);

pub fn framed<T, U>(inner: T, codec: U) -> Framed<T, U> {
    Framed {
        inner: framed_read2(framed_write2(Fuse(inner, codec))),
    }
}

impl<T, U> Stream for Framed<T, U>
    where T: AsyncRead,
          U: Decoder,
          U::Error: From<io::Error>
{
    type Item = U::Item;
    type Error = U::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

impl<T, U> Sink for Framed<T, U>
    where T: AsyncWrite,
          U: Encoder,
          U::Error: From<io::Error>
{
    type SinkItem = U::Item;
    type SinkError = U::Error;

    fn start_send(&mut self,
                  item: Self::SinkItem)
                  -> StartSend<Self::SinkItem, Self::SinkError>
    {
        self.inner.get_mut().start_send(item)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.inner.get_mut().poll_complete()
    }
}

// ===== impl Fuse =====

impl<T: Read, U> Read for Fuse<T, U> {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        self.0.read(dst)
    }
}

impl<T: AsyncRead, U> AsyncRead for Fuse<T, U> {
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [u8]) -> bool {
        self.0.prepare_uninitialized_buffer(buf)
    }
}

impl<T: Write, U> Write for Fuse<T, U> {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        self.0.write(src)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<T: AsyncWrite, U> AsyncWrite for Fuse<T, U> {
}

impl<T, U: Decoder> Decoder for Fuse<T, U> {
    type Item = U::Item;
    type Error = U::Error;

    fn decode(&mut self, buffer: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.1.decode(buffer)
    }

    fn eof(&mut self, buffer: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.1.eof(buffer)
    }
}

impl<T, U: Encoder> Encoder for Fuse<T, U> {
    type Item = U::Item;
    type Error = U::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.1.encode(item, dst)
    }
}
