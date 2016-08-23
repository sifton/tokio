//! A dispatcher for pipelining protocols
//!
//! This module contains reusable components for quickly implementing clients
//! and servers for pipeline based protocols.
//!
//! # Pipelining
//!
//! Protocol pipelining is a technique in which multiple requests are written
//! out to a single destination without waiting for their corresponding
//! responses. Pipelining is used in a multitude of different protocols such as
//! HTTP/1.1 and Redis in order to increase throughput on a single connection.
//!
//! Pipelining with the max number of in-flight requests set to 1 implies that
//! for each request, the response must be received before sending another
//! request on the same connection.
//!
//! Another protocol dispatching strategy is multiplexing (which will be
//! included in Tokio soon).
//!
//! # Usage
//!
//! Both the server and client pipeline dispatchers take a generic `Transport`
//! that reads and writes `Frame` messages. It operates on the transport
//! following the rules of pipelining as described above and exposes the
//! protocol using a `Service`.

mod client;
mod server;
mod pipeline;

pub use self::client::{connect, ClientHandle};
pub use self::server::Server;

use Service;
use io::{Readiness};
use util::future::Sender;
use futures::Future;
use futures::stream::Stream;
use take::Take;
use std::{fmt, io};

/// A pipelined protocol frame
#[derive(PartialEq, Eq, Clone)]
pub enum Frame<T, E, B = ()> {
    /// Either a request or a response
    Message(T),
    /// Body frame. None indicates that the body is done streaming.
    Body(Option<B>),
    /// Error
    Error(E),
    /// Final frame sent in each transport direction
    Done,
}

/// Frame read from a pipeline transport
pub type OutFrame<T, E, B = ()> = Frame<(T, Option<Sender<B, E>>), E, B>;

/// Error returned as an Error frame or an io::Error that occurerred during
/// normal processing of the Transport
pub enum Error<E> {
    /// Transport frame level error
    Transport(E),
    /// I/O level error
    Io(io::Error),
}

/// A specialization of `Service` supporting the requirements of server
/// pipelined services
///
/// `Service` should be implemented instead of this trait.
pub trait ServerService: Send + 'static {
    /// Requests handled by the service.
    type Req: Send + 'static;

    /// Responses given by the service.
    type Resp: Send + 'static;

    /// Response body chunk
    type Body: Send + 'static;

    /// Response body stream
    type BodyStream: Stream<Item = Self::Body, Error = Self::Error>;

    /// Errors produced by the service.
    type Error: Send + 'static;

    /// The future response value.
    type Fut: Future<Item = (Self::Resp, Option<Self::BodyStream>), Error = Self::Error>;

    /// Process the request and return the response asynchronously.
    fn call(&self, req: Self::Req) -> Self::Fut;
}

/// A specialization of `io::Transport` supporting the requirements of
/// pipeline based protocols.
///
/// `io::Transport` should be implemented instead of this trait.
pub trait Transport: Readiness {
    /// Messages written to the transport
    type In: Send + 'static;

    /// Inbound body frame
    type BodyIn: Send + 'static;

    /// Messages read from the transport
    type Out: Send + 'static;

    /// Outbound body frame
    type BodyOut: Send + 'static;

    /// Transport error
    type Error: Send + 'static; // TODO: rename

    /// Read a message from the `Transport`
    fn read(&mut self) -> io::Result<Option<OutFrame<Self::Out, Self::Error, Self::BodyOut>>>;

    /// Write a message to the `Transport`
    fn write(&mut self, req: Frame<Self::In, Self::Error, Self::BodyIn>) -> io::Result<Option<()>>;

    /// Flush pending writes to the socket
    fn flush(&mut self) -> io::Result<Option<()>>;
}

/// A specialization of `io::NewTransport` supporting the requirements of
/// pipeline based protocols.
///
/// `io::NewTransport` should be implemented instead of this trait.
pub trait NewTransport: Send + 'static {
    /// Messages written to the transport
    type In: Send + 'static;

    /// Inbound streaming body
    type BodyIn: Send + 'static;

    /// Messages read from the transport
    type Out: Send + 'static;

    /// Outbound streaming body
    type BodyOut: Send + 'static;

    /// Errors
    type Error: Send + 'static;

    /// Transport returned
    type Item: Transport<In = Self::In,
                     BodyIn = Self::BodyIn,
                        Out = Self::Out,
                    BodyOut = Self::BodyOut,
                      Error = Self::Error>;

    /// Create and return a new `Transport`
    fn new_transport(&self) -> io::Result<Self::Item>;
}

impl<T, E, B> Frame<T, E, B>
{
    /// Unwraps a frame, yielding the content of the `Message`.
    pub fn unwrap_msg(self) -> T {
        match self {
            Frame::Message(v) => v,
            Frame::Body(..) => panic!("called `Frame::unwrap_msg()` on a `Body` value"),
            Frame::Error(..) => panic!("called `Frame::unwrap_msg()` on an `Error` value"),
            Frame::Done => panic!("called `Frame::unwrap_msg()` on a `Done` value"),
        }
    }

    /// Unwraps a frame, yielding the content of the `Body`.
    pub fn unwrap_body(self) -> Option<B> {
        match self {
            Frame::Body(v) => v,
            Frame::Message(..) => panic!("called `Frame::unwrap_body()` on a `Message` value"),
            Frame::Error(..) => panic!("called `Frame::unwrap_body()` on an `Error` value"),
            Frame::Done => panic!("called `Frame::unwrap_body()` on a `Done` value"),
        }
    }

    /// Unwraps a frame, yielding the content of the `Error`.
    pub fn unwrap_err(self) -> E {
        match self {
            Frame::Error(e) => e,
            Frame::Body(..) => panic!("called `Frame::unwrap_err()` on a `Body` value"),
            Frame::Message(..) => panic!("called `Frame::unwrap_err()` on a `Message` value"),
            Frame::Done => panic!("called `Frame::unwrap_message()` on a `Done` value"),
        }
    }

    /// Returns true if the frame is `Frame::Done`
    pub fn is_done(&self) -> bool {
        match *self {
            Frame::Done => true,
            _ => false,
        }
    }
}

impl<T, E, B> fmt::Debug for Frame<T, E, B>
    where T: fmt::Debug,
          E: fmt::Debug,
          B: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Frame::Message(ref v) => write!(fmt, "Frame::Message({:?})", v),
            Frame::Body(ref v) => write!(fmt, "Frame::Body({:?})", v),
            Frame::Error(ref v) => write!(fmt, "Frame::Error({:?})", v),
            Frame::Done => write!(fmt, "Frame::Done"),
        }
    }
}

impl<S, Resp, Body, BodyStream> ServerService for S
    where S: Service<Resp = (Resp, Option<BodyStream>)>,
          Resp: Send + 'static,
          Body: Send + 'static,
          BodyStream: Stream<Item = Body, Error = S::Error>,
{
    type Req = S::Req;
    type Resp = Resp;
    type Body = Body;
    type BodyStream = BodyStream;
    type Error = S::Error;
    type Fut = S::Fut;

    fn call(&self, req: Self::Req) -> Self::Fut {
        Service::call(self, req)
    }
}

impl<T, M1, M2, B1, B2, E> Transport for T
    where T: ::io::Transport<In = Frame<M1, E, B1>, Out = OutFrame<M2, E, B2>>,
          M1: Send + 'static,
          B1: Send + 'static,
          M2: Send + 'static,
          B2: Send + 'static,
          E: Send + 'static,
{
    type In = M1;
    type BodyIn = B1;
    type Out = M2;
    type BodyOut = B2;
    type Error = E;

    fn read(&mut self) -> io::Result<Option<OutFrame<M2, E, B2>>> {
        ::io::Transport::read(self)
    }

    fn write(&mut self, req: Frame<M1, E, B1>) -> io::Result<Option<()>> {
        ::io::Transport::write(self, req)
    }

    fn flush(&mut self) -> io::Result<Option<()>> {
        ::io::Transport::flush(self)
    }
}

impl<F, T> NewTransport for F
    where F: Fn() -> io::Result<T> + Send + 'static,
          T: Transport,
{
    type In = T::In;
    type BodyIn = T::BodyIn;
    type Out = T::Out;
    type BodyOut = T::BodyOut;
    type Error = T::Error;
    type Item = T;

    fn new_transport(&self) -> io::Result<T> {
        self()
    }
}

impl<F, T> NewTransport for Take<F>
    where F: FnOnce() -> io::Result<T> + Send + 'static,
          T: Transport,
{
    type In = T::In;
    type BodyIn = T::BodyIn;
    type Out = T::Out;
    type BodyOut = T::BodyOut;
    type Error = T::Error;
    type Item = T;

    fn new_transport(&self) -> io::Result<T> {
        self.take()()
    }
}

impl From<Error<io::Error>> for io::Error {
    fn from(err: Error<io::Error>) -> Self {
        match err {
            Error::Transport(e) => e,
            Error::Io(e) => e,
        }
    }
}
