use std::collections::VecDeque;
use std::time::Instant;
use std::{fmt, io};

use actix_codec::{AsyncRead, AsyncWrite, Decoder, Encoder};
use actix_service::Service;
use actix_utils::cloneable::CloneableService;
use bitflags::bitflags;
use bytes::{BufMut, BytesMut};
use futures::{Async, Future, Poll};
use log::{error, trace};
use tokio_timer::Delay;

use crate::body::{Body, BodySize, MessageBody, ResponseBody};
use crate::config::ServiceConfig;
use crate::error::{DispatchError, Error};
use crate::error::{ParseError, PayloadError};
use crate::request::Request;
use crate::response::Response;

use super::codec::Codec;
use super::payload::{Payload, PayloadSender, PayloadStatus};
use super::{Message, MessageType};

const LW_BUFFER_SIZE: usize = 4096;
const HW_BUFFER_SIZE: usize = 32_768;
const MAX_PIPELINED_MESSAGES: usize = 16;

bitflags! {
    pub struct Flags: u8 {
        const STARTED            = 0b0000_0001;
        const KEEPALIVE          = 0b0000_0010;
        const POLLED             = 0b0000_0100;
        const SHUTDOWN           = 0b0000_1000;
        const READ_DISCONNECT    = 0b0001_0000;
        const WRITE_DISCONNECT   = 0b0010_0000;
        const DROPPING           = 0b0100_0000;
    }
}

/// Dispatcher for HTTP/1.1 protocol
pub struct Dispatcher<T, S, B, X>
where
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
{
    inner: Option<InnerDispatcher<T, S, B, X>>,
}

struct InnerDispatcher<T, S, B, X>
where
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
{
    service: CloneableService<S>,
    expect: CloneableService<X>,
    flags: Flags,
    error: Option<DispatchError>,

    state: State<S, B, X>,
    payload: Option<PayloadSender>,
    messages: VecDeque<DispatcherMessage>,

    ka_expire: Instant,
    ka_timer: Option<Delay>,

    io: T,
    read_buf: BytesMut,
    write_buf: BytesMut,
    codec: Codec,
}

enum DispatcherMessage {
    Item(Request),
    Error(Response<()>),
}

enum State<S, B, X>
where
    S: Service<Request = Request>,
    X: Service<Request = Request, Response = Request>,
    B: MessageBody,
{
    None,
    ExpectCall(X::Future),
    ServiceCall(S::Future),
    SendPayload(ResponseBody<B>),
}

impl<S, B, X> State<S, B, X>
where
    S: Service<Request = Request>,
    X: Service<Request = Request, Response = Request>,
    B: MessageBody,
{
    fn is_empty(&self) -> bool {
        if let State::None = self {
            true
        } else {
            false
        }
    }

    fn is_call(&self) -> bool {
        if let State::ServiceCall(_) = self {
            true
        } else {
            false
        }
    }
}

impl<S, B, X> fmt::Debug for State<S, B, X>
where
    S: Service<Request = Request>,
    X: Service<Request = Request, Response = Request>,
    B: MessageBody,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            State::None => write!(f, "State::None"),
            State::ExpectCall(_) => write!(f, "State::ExceptCall"),
            State::ServiceCall(_) => write!(f, "State::ServiceCall"),
            State::SendPayload(_) => write!(f, "State::SendPayload"),
        }
    }
}

impl<T, S, B, X> Dispatcher<T, S, B, X>
where
    T: AsyncRead + AsyncWrite,
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
{
    /// Create http/1 dispatcher.
    pub fn new(
        stream: T,
        config: ServiceConfig,
        service: CloneableService<S>,
        expect: CloneableService<X>,
    ) -> Self {
        Dispatcher::with_timeout(
            stream,
            Codec::new(config.clone()),
            config,
            BytesMut::with_capacity(HW_BUFFER_SIZE),
            None,
            service,
            expect,
        )
    }

    /// Create http/1 dispatcher with slow request timeout.
    pub fn with_timeout(
        io: T,
        codec: Codec,
        config: ServiceConfig,
        read_buf: BytesMut,
        timeout: Option<Delay>,
        service: CloneableService<S>,
        expect: CloneableService<X>,
    ) -> Self {
        let keepalive = config.keep_alive_enabled();
        let flags = if keepalive {
            Flags::KEEPALIVE
        } else {
            Flags::empty()
        };

        // keep-alive timer
        let (ka_expire, ka_timer) = if let Some(delay) = timeout {
            (delay.deadline(), Some(delay))
        } else if let Some(delay) = config.keep_alive_timer() {
            (delay.deadline(), Some(delay))
        } else {
            (config.now(), None)
        };

        Dispatcher {
            inner: Some(InnerDispatcher {
                io,
                codec,
                read_buf,
                write_buf: BytesMut::with_capacity(HW_BUFFER_SIZE),
                payload: None,
                state: State::None,
                error: None,
                messages: VecDeque::new(),
                service,
                expect,
                flags,
                ka_expire,
                ka_timer,
            }),
        }
    }
}

impl<T, S, B, X> InnerDispatcher<T, S, B, X>
where
    T: AsyncRead + AsyncWrite,
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
{
    fn can_read(&self) -> bool {
        if self.flags.contains(Flags::READ_DISCONNECT) {
            return false;
        } else if let Some(ref info) = self.payload {
            info.need_read() == PayloadStatus::Read
        } else {
            true
        }
    }

    // if checked is set to true, delay disconnect until all tasks have finished.
    fn client_disconnected(&mut self) {
        self.flags
            .insert(Flags::READ_DISCONNECT | Flags::WRITE_DISCONNECT);
        if let Some(mut payload) = self.payload.take() {
            payload.set_error(PayloadError::Incomplete(None));
        }
    }

    /// Flush stream
    ///
    /// true - got whouldblock
    /// false - didnt get whouldblock
    fn poll_flush(&mut self) -> Result<bool, DispatchError> {
        if self.write_buf.is_empty() {
            return Ok(false);
        }

        let len = self.write_buf.len();
        let mut written = 0;
        while written < len {
            match self.io.write(&self.write_buf[written..]) {
                Ok(0) => {
                    return Err(DispatchError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "",
                    )));
                }
                Ok(n) => {
                    written += n;
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if written > 0 {
                        let _ = self.write_buf.split_to(written);
                    }
                    return Ok(true);
                }
                Err(err) => return Err(DispatchError::Io(err)),
            }
        }
        if written > 0 {
            if written == self.write_buf.len() {
                unsafe { self.write_buf.set_len(0) }
            } else {
                let _ = self.write_buf.split_to(written);
            }
        }
        Ok(false)
    }

    fn send_response(
        &mut self,
        message: Response<()>,
        body: ResponseBody<B>,
    ) -> Result<State<S, B, X>, DispatchError> {
        self.codec
            .encode(Message::Item((message, body.length())), &mut self.write_buf)
            .map_err(|err| {
                if let Some(mut payload) = self.payload.take() {
                    payload.set_error(PayloadError::Incomplete(None));
                }
                DispatchError::Io(err)
            })?;

        self.flags.set(Flags::KEEPALIVE, self.codec.keepalive());
        match body.length() {
            BodySize::None | BodySize::Empty => Ok(State::None),
            _ => Ok(State::SendPayload(body)),
        }
    }

    fn send_continue(&mut self) {
        self.write_buf
            .extend_from_slice(b"HTTP/1.1 100 Continue\r\n\r\n");
    }

    fn poll_response(&mut self) -> Result<bool, DispatchError> {
        loop {
            let state = match self.state {
                State::None => match self.messages.pop_front() {
                    Some(DispatcherMessage::Item(req)) => {
                        Some(self.handle_request(req)?)
                    }
                    Some(DispatcherMessage::Error(res)) => {
                        Some(self.send_response(res, ResponseBody::Other(Body::Empty))?)
                    }
                    None => None,
                },
                State::ExpectCall(ref mut fut) => match fut.poll() {
                    Ok(Async::Ready(req)) => {
                        self.send_continue();
                        self.state = State::ServiceCall(self.service.call(req));
                        continue;
                    }
                    Ok(Async::NotReady) => None,
                    Err(e) => {
                        let res: Response = e.into().into();
                        let (res, body) = res.replace_body(());
                        Some(self.send_response(res, body.into_body())?)
                    }
                },
                State::ServiceCall(ref mut fut) => match fut.poll() {
                    Ok(Async::Ready(res)) => {
                        let (res, body) = res.into().replace_body(());
                        self.state = self.send_response(res, body)?;
                        continue;
                    }
                    Ok(Async::NotReady) => None,
                    Err(e) => {
                        let res: Response = e.into().into();
                        let (res, body) = res.replace_body(());
                        Some(self.send_response(res, body.into_body())?)
                    }
                },
                State::SendPayload(ref mut stream) => {
                    loop {
                        if self.write_buf.len() < HW_BUFFER_SIZE {
                            match stream
                                .poll_next()
                                .map_err(|_| DispatchError::Unknown)?
                            {
                                Async::Ready(Some(item)) => {
                                    self.codec.encode(
                                        Message::Chunk(Some(item)),
                                        &mut self.write_buf,
                                    )?;
                                    continue;
                                }
                                Async::Ready(None) => {
                                    self.codec.encode(
                                        Message::Chunk(None),
                                        &mut self.write_buf,
                                    )?;
                                    self.state = State::None;
                                }
                                Async::NotReady => return Ok(false),
                            }
                        } else {
                            return Ok(true);
                        }
                        break;
                    }
                    continue;
                }
            };

            // set new state
            if let Some(state) = state {
                self.state = state;
                if !self.state.is_empty() {
                    continue;
                }
            } else {
                // if read-backpressure is enabled and we consumed some data.
                // we may read more data and retry
                if self.state.is_call() {
                    if self.poll_request()? {
                        continue;
                    }
                } else if !self.messages.is_empty() {
                    continue;
                }
            }
            break;
        }

        Ok(false)
    }

    fn handle_request(&mut self, req: Request) -> Result<State<S, B, X>, DispatchError> {
        // Handle `EXPECT: 100-Continue` header
        let req = if req.head().expect() {
            let mut task = self.expect.call(req);
            match task.poll() {
                Ok(Async::Ready(req)) => {
                    self.send_continue();
                    req
                }
                Ok(Async::NotReady) => return Ok(State::ExpectCall(task)),
                Err(e) => {
                    let e = e.into();
                    let res: Response = e.into();
                    let (res, body) = res.replace_body(());
                    return self.send_response(res, body.into_body());
                }
            }
        } else {
            req
        };

        // Call service
        let mut task = self.service.call(req);
        match task.poll() {
            Ok(Async::Ready(res)) => {
                let (res, body) = res.into().replace_body(());
                self.send_response(res, body)
            }
            Ok(Async::NotReady) => Ok(State::ServiceCall(task)),
            Err(e) => {
                let res: Response = e.into().into();
                let (res, body) = res.replace_body(());
                self.send_response(res, body.into_body())
            }
        }
    }

    /// Process one incoming requests
    pub(self) fn poll_request(&mut self) -> Result<bool, DispatchError> {
        // limit a mount of non processed requests
        if self.messages.len() >= MAX_PIPELINED_MESSAGES || !self.can_read() {
            return Ok(false);
        }

        let mut updated = false;
        loop {
            match self.codec.decode(&mut self.read_buf) {
                Ok(Some(msg)) => {
                    updated = true;
                    self.flags.insert(Flags::STARTED);

                    match msg {
                        Message::Item(mut req) => {
                            match self.codec.message_type() {
                                MessageType::Payload | MessageType::Stream => {
                                    let (ps, pl) = Payload::create(false);
                                    let (req1, _) =
                                        req.replace_payload(crate::Payload::H1(pl));
                                    req = req1;
                                    self.payload = Some(ps);
                                }
                                _ => (),
                            }

                            // handle request early
                            if self.state.is_empty() {
                                self.state = self.handle_request(req)?;
                            } else {
                                self.messages.push_back(DispatcherMessage::Item(req));
                            }
                        }
                        Message::Chunk(Some(chunk)) => {
                            if let Some(ref mut payload) = self.payload {
                                payload.feed_data(chunk);
                            } else {
                                error!(
                                    "Internal server error: unexpected payload chunk"
                                );
                                self.flags.insert(Flags::READ_DISCONNECT);
                                self.messages.push_back(DispatcherMessage::Error(
                                    Response::InternalServerError().finish().drop_body(),
                                ));
                                self.error = Some(DispatchError::InternalError);
                                break;
                            }
                        }
                        Message::Chunk(None) => {
                            if let Some(mut payload) = self.payload.take() {
                                payload.feed_eof();
                            } else {
                                error!("Internal server error: unexpected eof");
                                self.flags.insert(Flags::READ_DISCONNECT);
                                self.messages.push_back(DispatcherMessage::Error(
                                    Response::InternalServerError().finish().drop_body(),
                                ));
                                self.error = Some(DispatchError::InternalError);
                                break;
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(ParseError::Io(e)) => {
                    self.client_disconnected();
                    self.error = Some(DispatchError::Io(e));
                    break;
                }
                Err(e) => {
                    if let Some(mut payload) = self.payload.take() {
                        payload.set_error(PayloadError::EncodingCorrupted);
                    }

                    // Malformed requests should be responded with 400
                    self.messages.push_back(DispatcherMessage::Error(
                        Response::BadRequest().finish().drop_body(),
                    ));
                    self.flags.insert(Flags::READ_DISCONNECT);
                    self.error = Some(e.into());
                    break;
                }
            }
        }

        if updated && self.ka_timer.is_some() {
            if let Some(expire) = self.codec.config.keep_alive_expire() {
                self.ka_expire = expire;
            }
        }
        Ok(updated)
    }

    /// keep-alive timer
    fn poll_keepalive(&mut self) -> Result<(), DispatchError> {
        if self.ka_timer.is_none() {
            // shutdown timeout
            if self.flags.contains(Flags::SHUTDOWN) {
                if let Some(interval) = self.codec.config.client_disconnect_timer() {
                    self.ka_timer = Some(Delay::new(interval));
                } else {
                    self.flags.insert(Flags::READ_DISCONNECT);
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        }

        match self.ka_timer.as_mut().unwrap().poll().map_err(|e| {
            error!("Timer error {:?}", e);
            DispatchError::Unknown
        })? {
            Async::Ready(_) => {
                // if we get timeout during shutdown, drop connection
                if self.flags.contains(Flags::SHUTDOWN) {
                    return Err(DispatchError::DisconnectTimeout);
                } else if self.ka_timer.as_mut().unwrap().deadline() >= self.ka_expire {
                    // check for any outstanding tasks
                    if self.state.is_empty() && self.write_buf.is_empty() {
                        if self.flags.contains(Flags::STARTED) {
                            trace!("Keep-alive timeout, close connection");
                            self.flags.insert(Flags::SHUTDOWN);

                            // start shutdown timer
                            if let Some(deadline) =
                                self.codec.config.client_disconnect_timer()
                            {
                                if let Some(timer) = self.ka_timer.as_mut() {
                                    timer.reset(deadline);
                                    let _ = timer.poll();
                                }
                            } else {
                                // no shutdown timeout, drop socket
                                self.flags.insert(Flags::WRITE_DISCONNECT);
                                return Ok(());
                            }
                        } else {
                            // timeout on first request (slow request) return 408
                            if !self.flags.contains(Flags::STARTED) {
                                trace!("Slow request timeout");
                                let _ = self.send_response(
                                    Response::RequestTimeout().finish().drop_body(),
                                    ResponseBody::Other(Body::Empty),
                                );
                            } else {
                                trace!("Keep-alive connection timeout");
                            }
                            self.flags.insert(Flags::STARTED | Flags::SHUTDOWN);
                            self.state = State::None;
                        }
                    } else if let Some(deadline) = self.codec.config.keep_alive_expire()
                    {
                        if let Some(timer) = self.ka_timer.as_mut() {
                            timer.reset(deadline);
                            let _ = timer.poll();
                        }
                    }
                } else if let Some(timer) = self.ka_timer.as_mut() {
                    timer.reset(self.ka_expire);
                    let _ = timer.poll();
                }
            }
            Async::NotReady => (),
        }

        Ok(())
    }
}

impl<T, S, B, X> Future for Dispatcher<T, S, B, X>
where
    T: AsyncRead + AsyncWrite,
    S: Service<Request = Request>,
    S::Error: Into<Error>,
    S::Response: Into<Response<B>>,
    B: MessageBody,
    X: Service<Request = Request, Response = Request>,
    X::Error: Into<Error>,
{
    type Item = ();
    type Error = DispatchError;

    #[inline]
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let inner = self.inner.as_mut().unwrap();
        inner.poll_keepalive()?;

        if inner.flags.contains(Flags::SHUTDOWN) {
            if inner.flags.contains(Flags::WRITE_DISCONNECT) {
                Ok(Async::Ready(()))
            } else {
                // flush buffer
                inner.poll_flush()?;
                if !inner.write_buf.is_empty() {
                    Ok(Async::NotReady)
                } else {
                    match inner.io.shutdown()? {
                        Async::Ready(_) => Ok(Async::Ready(())),
                        Async::NotReady => Ok(Async::NotReady),
                    }
                }
            }
        } else {
            // read socket into a buf
            if !inner.flags.contains(Flags::READ_DISCONNECT) {
                if let Some(true) = read_available(&mut inner.io, &mut inner.read_buf)? {
                    inner.flags.insert(Flags::READ_DISCONNECT)
                }
            }

            inner.poll_request()?;
            loop {
                if inner.write_buf.remaining_mut() < LW_BUFFER_SIZE {
                    inner.write_buf.reserve(HW_BUFFER_SIZE);
                }
                let need_write = inner.poll_response()?;

                // we didnt get WouldBlock from write operation,
                // so data get written to kernel completely (OSX)
                // and we have to write again otherwise response can get stuck
                if inner.poll_flush()? || !need_write {
                    break;
                }
            }

            // client is gone
            if inner.flags.contains(Flags::WRITE_DISCONNECT) {
                return Ok(Async::Ready(()));
            }

            let is_empty = inner.state.is_empty();

            // read half is closed and we do not processing any responses
            if inner.flags.contains(Flags::READ_DISCONNECT) && is_empty {
                inner.flags.insert(Flags::SHUTDOWN);
            }

            // keep-alive and stream errors
            if is_empty && inner.write_buf.is_empty() {
                if let Some(err) = inner.error.take() {
                    Err(err)
                }
                // disconnect if keep-alive is not enabled
                else if inner.flags.contains(Flags::STARTED)
                    && !inner.flags.intersects(Flags::KEEPALIVE)
                {
                    inner.flags.insert(Flags::SHUTDOWN);
                    self.poll()
                }
                // disconnect if shutdown
                else if inner.flags.contains(Flags::SHUTDOWN) {
                    self.poll()
                } else {
                    Ok(Async::NotReady)
                }
            } else {
                Ok(Async::NotReady)
            }
        }
    }
}

fn read_available<T>(io: &mut T, buf: &mut BytesMut) -> Result<Option<bool>, io::Error>
where
    T: io::Read,
{
    let mut read_some = false;
    loop {
        if buf.remaining_mut() < LW_BUFFER_SIZE {
            buf.reserve(HW_BUFFER_SIZE);
        }

        let read = unsafe { io.read(buf.bytes_mut()) };
        match read {
            Ok(n) => {
                if n == 0 {
                    return Ok(Some(true));
                } else {
                    read_some = true;
                    unsafe {
                        buf.advance_mut(n);
                    }
                }
            }
            Err(e) => {
                return if e.kind() == io::ErrorKind::WouldBlock {
                    if read_some {
                        Ok(Some(false))
                    } else {
                        Ok(None)
                    }
                } else if e.kind() == io::ErrorKind::ConnectionReset && read_some {
                    Ok(Some(true))
                } else {
                    Err(e)
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cmp, io};

    use actix_codec::{AsyncRead, AsyncWrite};
    use actix_service::IntoService;
    use bytes::{Buf, Bytes, BytesMut};
    use futures::future::{lazy, ok};

    use super::*;
    use crate::error::Error;
    use crate::h1::ExpectHandler;

    struct Buffer {
        buf: Bytes,
        write_buf: BytesMut,
        err: Option<io::Error>,
    }

    impl Buffer {
        fn new(data: &'static str) -> Buffer {
            Buffer {
                buf: Bytes::from(data),
                write_buf: BytesMut::new(),
                err: None,
            }
        }
    }

    impl AsyncRead for Buffer {}
    impl io::Read for Buffer {
        fn read(&mut self, dst: &mut [u8]) -> Result<usize, io::Error> {
            if self.buf.is_empty() {
                if self.err.is_some() {
                    Err(self.err.take().unwrap())
                } else {
                    Err(io::Error::new(io::ErrorKind::WouldBlock, ""))
                }
            } else {
                let size = cmp::min(self.buf.len(), dst.len());
                let b = self.buf.split_to(size);
                dst[..size].copy_from_slice(&b);
                Ok(size)
            }
        }
    }

    impl io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.write_buf.extend(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl AsyncWrite for Buffer {
        fn shutdown(&mut self) -> Poll<(), io::Error> {
            Ok(Async::Ready(()))
        }
        fn write_buf<B: Buf>(&mut self, _: &mut B) -> Poll<usize, io::Error> {
            Ok(Async::NotReady)
        }
    }

    #[test]
    fn test_req_parse_err() {
        let mut sys = actix_rt::System::new("test");
        let _ = sys.block_on(lazy(|| {
            let buf = Buffer::new("GET /test HTTP/1\r\n\r\n");

            let mut h1 = Dispatcher::new(
                buf,
                ServiceConfig::default(),
                CloneableService::new(
                    (|_| ok::<_, Error>(Response::Ok().finish())).into_service(),
                ),
                CloneableService::new(ExpectHandler),
            );
            assert!(h1.poll().is_err());
            assert!(h1
                .inner
                .as_ref()
                .unwrap()
                .flags
                .contains(Flags::READ_DISCONNECT));
            assert_eq!(
                &h1.inner.as_ref().unwrap().io.write_buf[..26],
                b"HTTP/1.1 400 Bad Request\r\n"
            );
            ok::<_, ()>(())
        }));
    }
}
