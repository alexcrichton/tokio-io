//! Core I/O traits and combinators when working with Tokio.
//!
//! A description of the high-level I/O combinators can be [found online] in
//! addition to a description of the [low level details].
//!
//! [found online]: https://tokio.rs/docs/getting-started/core/
//! [low level details]: https://tokio.rs/docs/going-deeper/core-low-level/

#![deny(missing_docs)]
#![doc(html_root_url = "https://docs.rs/tokio-io/0.1")]

#[macro_use]
extern crate log;

#[macro_use]
extern crate futures;
extern crate bytes;

use std::io as std_io;

use futures::{BoxFuture, Poll, Async};
use futures::stream::BoxStream;

use bytes::{Buf, BufMut};

/// A convenience typedef around a `Future` whose error component is `io::Error`
pub type IoFuture<T> = BoxFuture<T, std_io::Error>;

/// A convenience typedef around a `Stream` whose error component is `io::Error`
pub type IoStream<T> = BoxStream<T, std_io::Error>;

/// A convenience macro for working with `io::Result<T>` from the `Read` and
/// `Write` traits.
///
/// This macro takes `io::Result<T>` as input, and returns `T` as the output. If
/// the input type is of the `Err` variant, then `Poll::NotReady` is returned if
/// it indicates `WouldBlock` or otherwise `Err` is returned.
#[macro_export]
macro_rules! try_nb {
    ($e:expr) => (match $e {
        Ok(t) => t,
        Err(ref e) if e.kind() == ::std::io::ErrorKind::WouldBlock => {
            return Ok(::futures::Async::NotReady)
        }
        Err(e) => return Err(e.into()),
    })
}

pub mod io;
pub mod codec;

mod copy;
mod flush;
mod framed;
mod framed_read;
mod framed_write;
mod lines;
mod read;
mod read_exact;
mod read_to_end;
mod read_until;
mod split;
mod window;
mod write_all;

use codec::{Decoder, Encoder, Framed};
use split::{ReadHalf, WriteHalf};

/// A trait for readable objects which operated in an asynchronous and
/// futures-aware fashion.
///
/// This trait inherits from `io::Read` and indicates as a marker that an I/O
/// object is **nonblocking**, meaning that it will return an error instead of
/// blocking when bytes are read. Specifically this means that the `read`
/// function for traits that implement this type can have a few return values:
///
/// * `Ok(n)` means that `n` bytes of data was immediately read and placed into
///   the output buffer, where `n` == 0 implies that EOF has been reached.
/// * `Err(e) if e.kind() == ErrorKind::WouldBlock` means that no data was read
///   into the buffer provided. The I/O object is not currently readable but may
///   become readable in the future. Most importantly, **the current future's
///   task is scheduled to get unparked when the object is readable**. This
///   means that like `Future::poll` you'll receive a notification when the I/O
///   object is readable again.
/// * `Err(e)` for other errors are standard I/O errors coming from the
///   underlying object.
///
/// This trait importantly means that the `read` method only works in the
/// context of a future's task. The object may panic if used outside of a task.
pub trait AsyncRead: std_io::Read {
    /// Prepares an uninitialized buffer to be safe to pass to `read`. Returns
    /// `true` if the supplied buffer was zeroed out.
    ///
    /// While it would be highly unusual, implementations of [`io::Read`] are
    /// able to read data from the buffer passed as an argument. Because of
    /// this, the buffer passed to [`io::Read`] must be initialized memory. In
    /// situations where large numbers of buffers are used, constantly having to
    /// zero out buffers can be expensive.
    ///
    /// This function does any necessary work to prepare an uninitialized buffer
    /// to be safe to pass to `read`. If `read` guarantees to never attempt read
    /// data out of the supplied buffer, then `prepare_uninitialized_buffer`
    /// doesn't need to do any work.
    ///
    /// If this function returns `true`, then the memory has been zeroed out.
    /// This allows implementations of `AsyncRead` which are composed of
    /// multiple sub implementations to efficiently implement
    /// `prepare_uninitialized_buffer`.
    ///
    /// This function is called from [`read_buf`].
    ///
    /// [`io::Read`]: https://doc.rust-lang.org/std/io/trait.Read.html
    /// [`read_buf`]: #method.read_buf
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [u8]) -> bool {
        for i in 0..buf.len() {
            buf[i] = 0;
        }

        true
    }

    /// Pull some bytes from this source into the specified `Buf`, returning
    /// how many bytes were read.
    fn read_buf<B: BufMut>(&mut self, buf: &mut B) -> Poll<usize, std_io::Error> {
        if !buf.has_remaining_mut() {
            return Ok(Async::Ready(0));
        }

        unsafe {
            let n = {
                let b = buf.bytes_mut();

                self.prepare_uninitialized_buffer(b);

                match self.read(b) {
                    Ok(n) => n,
                    Err(ref e) if e.kind() == std_io::ErrorKind::WouldBlock => {
                        return Ok(Async::NotReady);
                    }
                    Err(e) => return Err(e),
                }
            };

            buf.advance_mut(n);
            Ok(Async::Ready(n))
        }
    }

    /// Provides a `Stream` and `Sink` interface for reading and writing to this
    /// `Io` object, using `Decode` and `Encode` to read and write the raw data.
    ///
    /// Raw I/O objects work with byte sequences, but higher-level code usually
    /// wants to batch these into meaningful chunks, called "frames". This
    /// method layers framing on top of an I/O object, by using the `Codec`
    /// traits to handle encoding and decoding of messages frames. Note that
    /// the incoming and outgoing frame types may be distinct.
    ///
    /// This function returns a *single* object that is both `Stream` and
    /// `Sink`; grouping this into a single object is often useful for layering
    /// things like gzip or TLS, which require both read and write access to the
    /// underlying object.
    ///
    /// If you want to work more directly with the streams and sink, consider
    /// calling `split` on the `Framed` returned by this method, which will
    /// break them into separate objects, allowing them to interact more easily.
    fn framed<T: Encoder + Decoder>(self, codec: T) -> Framed<Self, T>
        where Self: AsyncWrite + Sized,
    {
        framed::framed(self, codec)
    }

    /// Helper method for splitting this read/write object into two halves.
    ///
    /// The two halves returned implement the `Read` and `Write` traits,
    /// respectively.
    fn split(self) -> (ReadHalf<Self>, WriteHalf<Self>)
        where Self: AsyncWrite + Sized,
    {
        split::split(self)
    }
}

impl<T: ?Sized + AsyncRead> AsyncRead for Box<T> {
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [u8]) -> bool {
        (**self).prepare_uninitialized_buffer(buf)
    }
}
impl<'a, T: ?Sized + AsyncRead> AsyncRead for &'a mut T {
    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [u8]) -> bool {
        (**self).prepare_uninitialized_buffer(buf)
    }
}

/// A trait for writable objects which operated in an asynchronous and
/// futures-aware fashion.
///
/// This trait inherits from `io::Write` and indicates as a marker that an I/O
/// object is **nonblocking**, meaning that it will return an error instead of
/// blocking when bytes are written. Specifically this means that the `write`
/// function for traits that implement this type can have a few return values:
///
/// * `Ok(n)` means that `n` bytes of data was immediately written .
/// * `Err(e) if e.kind() == ErrorKind::WouldBlock` means that no data was
///   written from the buffer provided. The I/O object is not currently
///   writable but may become writable in the future. Most importantly, **the
///   current future's task is scheduled to get unparked when the object is
///   readable**. This means that like `Future::poll` you'll receive a
///   notification when the I/O object is writable again.
/// * `Err(e)` for other errors are standard I/O errors coming from the
///   underlying object.
///
/// This trait importantly means that the `write` method only works in the
/// context of a future's task. The object may panic if used outside of a task.
pub trait AsyncWrite: std_io::Write {

    /// Write a `Buf` into this value, returning how many bytes were written.
    fn write_buf<B: Buf>(&mut self, buf: &mut B) -> Poll<usize, std_io::Error> {
        if !buf.has_remaining() {
            return Ok(Async::Ready(0));
        }

        match self.write(buf.bytes()) {
            Ok(n) => {
                buf.advance(n);
                Ok(Async::Ready(n))
            }
            Err(ref e) if e.kind() == std_io::ErrorKind::WouldBlock => {
                return Ok(Async::NotReady);
            }
            Err(e) => Err(e),
        }
    }
}

impl<T: ?Sized + AsyncWrite> AsyncWrite for Box<T> {
}
impl<'a, T: ?Sized + AsyncWrite> AsyncWrite for &'a mut T {
}

impl AsyncRead for std_io::Repeat {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [u8]) -> bool {
        false
    }
}

impl AsyncWrite for std_io::Sink {
}

// TODO: Implement `prepare_uninitialized_buffer` for `io::Take`.
// This is blocked on rust-lang/rust#27269
impl<T: AsyncRead> AsyncRead for std_io::Take<T> {
}
