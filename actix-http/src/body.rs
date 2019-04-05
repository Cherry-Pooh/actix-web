use std::marker::PhantomData;
use std::{fmt, mem};

use bytes::{Bytes, BytesMut};
use futures::{Async, Poll, Stream};

use crate::error::Error;

#[derive(Debug, PartialEq, Copy, Clone)]
/// Body size hint
pub enum BodySize {
    None,
    Empty,
    Sized(usize),
    Sized64(u64),
    Stream,
}

impl BodySize {
    pub fn is_eof(&self) -> bool {
        match self {
            BodySize::None
            | BodySize::Empty
            | BodySize::Sized(0)
            | BodySize::Sized64(0) => true,
            _ => false,
        }
    }
}

/// Type that provides this trait can be streamed to a peer.
pub trait MessageBody {
    fn length(&self) -> BodySize;

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error>;
}

impl MessageBody for () {
    fn length(&self) -> BodySize {
        BodySize::Empty
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        Ok(Async::Ready(None))
    }
}

impl<T: MessageBody> MessageBody for Box<T> {
    fn length(&self) -> BodySize {
        self.as_ref().length()
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        self.as_mut().poll_next()
    }
}

pub enum ResponseBody<B> {
    Body(B),
    Other(Body),
}

impl ResponseBody<Body> {
    pub fn into_body<B>(self) -> ResponseBody<B> {
        match self {
            ResponseBody::Body(b) => ResponseBody::Other(b),
            ResponseBody::Other(b) => ResponseBody::Other(b),
        }
    }
}

impl<B> ResponseBody<B> {
    pub fn take_body(&mut self) -> ResponseBody<B> {
        std::mem::replace(self, ResponseBody::Other(Body::None))
    }
}

impl<B: MessageBody> ResponseBody<B> {
    pub fn as_ref(&self) -> Option<&B> {
        if let ResponseBody::Body(ref b) = self {
            Some(b)
        } else {
            None
        }
    }
}

impl<B: MessageBody> MessageBody for ResponseBody<B> {
    fn length(&self) -> BodySize {
        match self {
            ResponseBody::Body(ref body) => body.length(),
            ResponseBody::Other(ref body) => body.length(),
        }
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        match self {
            ResponseBody::Body(ref mut body) => body.poll_next(),
            ResponseBody::Other(ref mut body) => body.poll_next(),
        }
    }
}

impl<B: MessageBody> Stream for ResponseBody<B> {
    type Item = Bytes;
    type Error = Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.poll_next()
    }
}

/// Represents various types of http message body.
pub enum Body {
    /// Empty response. `Content-Length` header is not set.
    None,
    /// Zero sized response body. `Content-Length` header is set to `0`.
    Empty,
    /// Specific response body.
    Bytes(Bytes),
    /// Generic message body.
    Message(Box<dyn MessageBody>),
}

impl Body {
    /// Create body from slice (copy)
    pub fn from_slice(s: &[u8]) -> Body {
        Body::Bytes(Bytes::from(s))
    }

    /// Create body from generic message body.
    pub fn from_message<B: MessageBody + 'static>(body: B) -> Body {
        Body::Message(Box::new(body))
    }
}

impl MessageBody for Body {
    fn length(&self) -> BodySize {
        match self {
            Body::None => BodySize::None,
            Body::Empty => BodySize::Empty,
            Body::Bytes(ref bin) => BodySize::Sized(bin.len()),
            Body::Message(ref body) => body.length(),
        }
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        match self {
            Body::None => Ok(Async::Ready(None)),
            Body::Empty => Ok(Async::Ready(None)),
            Body::Bytes(ref mut bin) => {
                let len = bin.len();
                if len == 0 {
                    Ok(Async::Ready(None))
                } else {
                    Ok(Async::Ready(Some(bin.split_to(len))))
                }
            }
            Body::Message(ref mut body) => body.poll_next(),
        }
    }
}

impl PartialEq for Body {
    fn eq(&self, other: &Body) -> bool {
        match *self {
            Body::None => match *other {
                Body::None => true,
                _ => false,
            },
            Body::Empty => match *other {
                Body::Empty => true,
                _ => false,
            },
            Body::Bytes(ref b) => match *other {
                Body::Bytes(ref b2) => b == b2,
                _ => false,
            },
            Body::Message(_) => false,
        }
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Body::None => write!(f, "Body::None"),
            Body::Empty => write!(f, "Body::Zero"),
            Body::Bytes(ref b) => write!(f, "Body::Bytes({:?})", b),
            Body::Message(_) => write!(f, "Body::Message(_)"),
        }
    }
}

impl From<&'static str> for Body {
    fn from(s: &'static str) -> Body {
        Body::Bytes(Bytes::from_static(s.as_ref()))
    }
}

impl From<&'static [u8]> for Body {
    fn from(s: &'static [u8]) -> Body {
        Body::Bytes(Bytes::from_static(s))
    }
}

impl From<Vec<u8>> for Body {
    fn from(vec: Vec<u8>) -> Body {
        Body::Bytes(Bytes::from(vec))
    }
}

impl From<String> for Body {
    fn from(s: String) -> Body {
        s.into_bytes().into()
    }
}

impl<'a> From<&'a String> for Body {
    fn from(s: &'a String) -> Body {
        Body::Bytes(Bytes::from(AsRef::<[u8]>::as_ref(&s)))
    }
}

impl From<Bytes> for Body {
    fn from(s: Bytes) -> Body {
        Body::Bytes(s)
    }
}

impl From<BytesMut> for Body {
    fn from(s: BytesMut) -> Body {
        Body::Bytes(s.freeze())
    }
}

impl MessageBody for Bytes {
    fn length(&self) -> BodySize {
        BodySize::Sized(self.len())
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        if self.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::Ready(Some(mem::replace(self, Bytes::new()))))
        }
    }
}

impl MessageBody for BytesMut {
    fn length(&self) -> BodySize {
        BodySize::Sized(self.len())
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        if self.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::Ready(Some(
                mem::replace(self, BytesMut::new()).freeze(),
            )))
        }
    }
}

impl MessageBody for &'static str {
    fn length(&self) -> BodySize {
        BodySize::Sized(self.len())
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        if self.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::Ready(Some(Bytes::from_static(
                mem::replace(self, "").as_ref(),
            ))))
        }
    }
}

impl MessageBody for &'static [u8] {
    fn length(&self) -> BodySize {
        BodySize::Sized(self.len())
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        if self.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::Ready(Some(Bytes::from_static(mem::replace(
                self, b"",
            )))))
        }
    }
}

impl MessageBody for Vec<u8> {
    fn length(&self) -> BodySize {
        BodySize::Sized(self.len())
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        if self.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::Ready(Some(Bytes::from(mem::replace(
                self,
                Vec::new(),
            )))))
        }
    }
}

impl MessageBody for String {
    fn length(&self) -> BodySize {
        BodySize::Sized(self.len())
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        if self.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::Ready(Some(Bytes::from(
                mem::replace(self, String::new()).into_bytes(),
            ))))
        }
    }
}

/// Type represent streaming body.
/// Response does not contain `content-length` header and appropriate transfer encoding is used.
pub struct BodyStream<S, E> {
    stream: S,
    _t: PhantomData<E>,
}

impl<S, E> BodyStream<S, E>
where
    S: Stream<Item = Bytes, Error = E>,
    E: Into<Error>,
{
    pub fn new(stream: S) -> Self {
        BodyStream {
            stream,
            _t: PhantomData,
        }
    }
}

impl<S, E> MessageBody for BodyStream<S, E>
where
    S: Stream<Item = Bytes, Error = E>,
    E: Into<Error>,
{
    fn length(&self) -> BodySize {
        BodySize::Stream
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        self.stream.poll().map_err(std::convert::Into::into)
    }
}

/// Type represent streaming body. This body implementation should be used
/// if total size of stream is known. Data get sent as is without using transfer encoding.
pub struct SizedStream<S> {
    size: usize,
    stream: S,
}

impl<S> SizedStream<S>
where
    S: Stream<Item = Bytes, Error = Error>,
{
    pub fn new(size: usize, stream: S) -> Self {
        SizedStream { size, stream }
    }
}

impl<S> MessageBody for SizedStream<S>
where
    S: Stream<Item = Bytes, Error = Error>,
{
    fn length(&self) -> BodySize {
        BodySize::Sized(self.size)
    }

    fn poll_next(&mut self) -> Poll<Option<Bytes>, Error> {
        self.stream.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Body {
        pub(crate) fn get_ref(&self) -> &[u8] {
            match *self {
                Body::Bytes(ref bin) => &bin,
                _ => panic!(),
            }
        }
    }

    impl ResponseBody<Body> {
        pub(crate) fn get_ref(&self) -> &[u8] {
            match *self {
                ResponseBody::Body(ref b) => b.get_ref(),
                ResponseBody::Other(ref b) => b.get_ref(),
            }
        }
    }

    #[test]
    fn test_static_str() {
        assert_eq!(Body::from("").length(), BodySize::Sized(0));
        assert_eq!(Body::from("test").length(), BodySize::Sized(4));
        assert_eq!(Body::from("test").get_ref(), b"test");
    }

    #[test]
    fn test_static_bytes() {
        assert_eq!(Body::from(b"test".as_ref()).length(), BodySize::Sized(4));
        assert_eq!(Body::from(b"test".as_ref()).get_ref(), b"test");
        assert_eq!(
            Body::from_slice(b"test".as_ref()).length(),
            BodySize::Sized(4)
        );
        assert_eq!(Body::from_slice(b"test".as_ref()).get_ref(), b"test");
    }

    #[test]
    fn test_vec() {
        assert_eq!(Body::from(Vec::from("test")).length(), BodySize::Sized(4));
        assert_eq!(Body::from(Vec::from("test")).get_ref(), b"test");
    }

    #[test]
    fn test_bytes() {
        assert_eq!(Body::from(Bytes::from("test")).length(), BodySize::Sized(4));
        assert_eq!(Body::from(Bytes::from("test")).get_ref(), b"test");
    }

    #[test]
    fn test_string() {
        let b = "test".to_owned();
        assert_eq!(Body::from(b.clone()).length(), BodySize::Sized(4));
        assert_eq!(Body::from(b.clone()).get_ref(), b"test");
        assert_eq!(Body::from(&b).length(), BodySize::Sized(4));
        assert_eq!(Body::from(&b).get_ref(), b"test");
    }

    #[test]
    fn test_bytes_mut() {
        let b = BytesMut::from("test");
        assert_eq!(Body::from(b.clone()).length(), BodySize::Sized(4));
        assert_eq!(Body::from(b).get_ref(), b"test");
    }
}
