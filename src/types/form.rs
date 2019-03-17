//! Form extractor

use std::rc::Rc;
use std::{fmt, ops};

use actix_http::error::{Error, PayloadError};
use actix_http::{HttpMessage, Payload};
use bytes::{Bytes, BytesMut};
use encoding::all::UTF_8;
use encoding::types::{DecoderTrap, Encoding};
use encoding::EncodingRef;
use futures::{Future, Poll, Stream};
use serde::de::DeserializeOwned;

use crate::error::UrlencodedError;
use crate::extract::FromRequest;
use crate::http::header::CONTENT_LENGTH;
use crate::request::HttpRequest;
use crate::service::ServiceFromRequest;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
/// Extract typed information from the request's body.
///
/// To extract typed information from request's body, the type `T` must
/// implement the `Deserialize` trait from *serde*.
///
/// [**FormConfig**](struct.FormConfig.html) allows to configure extraction
/// process.
///
/// ## Example
///
/// ```rust
/// # extern crate actix_web;
/// #[macro_use] extern crate serde_derive;
/// use actix_web::{web, App};
///
/// #[derive(Deserialize)]
/// struct FormData {
///     username: String,
/// }
///
/// /// Extract form data using serde.
/// /// This handler get called only if content type is *x-www-form-urlencoded*
/// /// and content of the request could be deserialized to a `FormData` struct
/// fn index(form: web::Form<FormData>) -> String {
///     format!("Welcome {}!", form.username)
/// }
/// # fn main() {}
/// ```
pub struct Form<T>(pub T);

impl<T> Form<T> {
    /// Deconstruct to an inner value
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> ops::Deref for Form<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> ops::DerefMut for Form<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T, P> FromRequest<P> for Form<T>
where
    T: DeserializeOwned + 'static,
    P: Stream<Item = Bytes, Error = crate::error::PayloadError> + 'static,
{
    type Error = Error;
    type Future = Box<Future<Item = Self, Error = Error>>;

    #[inline]
    fn from_request(req: &mut ServiceFromRequest<P>) -> Self::Future {
        let req2 = req.clone();
        let (limit, err) = req
            .route_data::<FormConfig>()
            .map(|c| (c.limit, c.ehandler.clone()))
            .unwrap_or((16384, None));

        Box::new(
            UrlEncoded::new(req)
                .limit(limit)
                .map_err(move |e| {
                    if let Some(err) = err {
                        (*err)(e, &req2)
                    } else {
                        e.into()
                    }
                })
                .map(Form),
        )
    }
}

impl<T: fmt::Debug> fmt::Debug for Form<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: fmt::Display> fmt::Display for Form<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Form extractor configuration
///
/// ```rust
/// #[macro_use] extern crate serde_derive;
/// use actix_web::{web, App, Result};
///
/// #[derive(Deserialize)]
/// struct FormData {
///     username: String,
/// }
///
/// /// Extract form data using serde.
/// /// Custom configuration is used for this handler, max payload size is 4k
/// fn index(form: web::Form<FormData>) -> Result<String> {
///     Ok(format!("Welcome {}!", form.username))
/// }
///
/// fn main() {
///     let app = App::new().service(
///         web::resource("/index.html")
///             .route(web::get()
///                 // change `Form` extractor configuration
///                 .data(web::FormConfig::default().limit(4097))
///                 .to(index))
///     );
/// }
/// ```
#[derive(Clone)]
pub struct FormConfig {
    limit: usize,
    ehandler: Option<Rc<Fn(UrlencodedError, &HttpRequest) -> Error>>,
}

impl FormConfig {
    /// Change max size of payload. By default max size is 16Kb
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Set custom error handler
    pub fn error_handler<F>(mut self, f: F) -> Self
    where
        F: Fn(UrlencodedError, &HttpRequest) -> Error + 'static,
    {
        self.ehandler = Some(Rc::new(f));
        self
    }
}

impl Default for FormConfig {
    fn default() -> Self {
        FormConfig {
            limit: 16384,
            ehandler: None,
        }
    }
}

/// Future that resolves to a parsed urlencoded values.
///
/// Parse `application/x-www-form-urlencoded` encoded request's body.
/// Return `UrlEncoded` future. Form can be deserialized to any type that
/// implements `Deserialize` trait from *serde*.
///
/// Returns error:
///
/// * content type is not `application/x-www-form-urlencoded`
/// * content-length is greater than 32k
///
pub struct UrlEncoded<T: HttpMessage, U> {
    stream: Payload<T::Stream>,
    limit: usize,
    length: Option<usize>,
    encoding: EncodingRef,
    err: Option<UrlencodedError>,
    fut: Option<Box<Future<Item = U, Error = UrlencodedError>>>,
}

impl<T, U> UrlEncoded<T, U>
where
    T: HttpMessage,
    T::Stream: Stream<Item = Bytes, Error = PayloadError>,
{
    /// Create a new future to URL encode a request
    pub fn new(req: &mut T) -> UrlEncoded<T, U> {
        // check content type
        if req.content_type().to_lowercase() != "application/x-www-form-urlencoded" {
            return Self::err(UrlencodedError::ContentType);
        }
        let encoding = match req.encoding() {
            Ok(enc) => enc,
            Err(_) => return Self::err(UrlencodedError::ContentType),
        };

        let mut len = None;
        if let Some(l) = req.headers().get(CONTENT_LENGTH) {
            if let Ok(s) = l.to_str() {
                if let Ok(l) = s.parse::<usize>() {
                    len = Some(l)
                } else {
                    return Self::err(UrlencodedError::UnknownLength);
                }
            } else {
                return Self::err(UrlencodedError::UnknownLength);
            }
        };

        UrlEncoded {
            encoding,
            stream: req.take_payload(),
            limit: 32_768,
            length: len,
            fut: None,
            err: None,
        }
    }

    fn err(e: UrlencodedError) -> Self {
        UrlEncoded {
            stream: Payload::None,
            limit: 32_768,
            fut: None,
            err: Some(e),
            length: None,
            encoding: UTF_8,
        }
    }

    /// Change max size of payload. By default max size is 256Kb
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

impl<T, U> Future for UrlEncoded<T, U>
where
    T: HttpMessage,
    T::Stream: Stream<Item = Bytes, Error = PayloadError> + 'static,
    U: DeserializeOwned + 'static,
{
    type Item = U;
    type Error = UrlencodedError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        if let Some(ref mut fut) = self.fut {
            return fut.poll();
        }

        if let Some(err) = self.err.take() {
            return Err(err);
        }

        // payload size
        let limit = self.limit;
        if let Some(len) = self.length.take() {
            if len > limit {
                return Err(UrlencodedError::Overflow);
            }
        }

        // future
        let encoding = self.encoding;
        let fut = std::mem::replace(&mut self.stream, Payload::None)
            .from_err()
            .fold(BytesMut::with_capacity(8192), move |mut body, chunk| {
                if (body.len() + chunk.len()) > limit {
                    Err(UrlencodedError::Overflow)
                } else {
                    body.extend_from_slice(&chunk);
                    Ok(body)
                }
            })
            .and_then(move |body| {
                if (encoding as *const Encoding) == UTF_8 {
                    serde_urlencoded::from_bytes::<U>(&body)
                        .map_err(|_| UrlencodedError::Parse)
                } else {
                    let body = encoding
                        .decode(&body, DecoderTrap::Strict)
                        .map_err(|_| UrlencodedError::Parse)?;
                    serde_urlencoded::from_str::<U>(&body)
                        .map_err(|_| UrlencodedError::Parse)
                }
            });
        self.fut = Some(Box::new(fut));
        self.poll()
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use serde::Deserialize;

    use super::*;
    use crate::http::header::CONTENT_TYPE;
    use crate::test::{block_on, TestRequest};

    #[derive(Deserialize, Debug, PartialEq)]
    struct Info {
        hello: String,
    }

    #[test]
    fn test_form() {
        let mut req =
            TestRequest::with_header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(CONTENT_LENGTH, "11")
                .set_payload(Bytes::from_static(b"hello=world"))
                .to_from();

        let s = block_on(Form::<Info>::from_request(&mut req)).unwrap();
        assert_eq!(s.hello, "world");
    }

    fn eq(err: UrlencodedError, other: UrlencodedError) -> bool {
        match err {
            UrlencodedError::Chunked => match other {
                UrlencodedError::Chunked => true,
                _ => false,
            },
            UrlencodedError::Overflow => match other {
                UrlencodedError::Overflow => true,
                _ => false,
            },
            UrlencodedError::UnknownLength => match other {
                UrlencodedError::UnknownLength => true,
                _ => false,
            },
            UrlencodedError::ContentType => match other {
                UrlencodedError::ContentType => true,
                _ => false,
            },
            _ => false,
        }
    }

    #[test]
    fn test_urlencoded_error() {
        let mut req =
            TestRequest::with_header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(CONTENT_LENGTH, "xxxx")
                .to_request();
        let info = block_on(UrlEncoded::<_, Info>::new(&mut req));
        assert!(eq(info.err().unwrap(), UrlencodedError::UnknownLength));

        let mut req =
            TestRequest::with_header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(CONTENT_LENGTH, "1000000")
                .to_request();
        let info = block_on(UrlEncoded::<_, Info>::new(&mut req));
        assert!(eq(info.err().unwrap(), UrlencodedError::Overflow));

        let mut req = TestRequest::with_header(CONTENT_TYPE, "text/plain")
            .header(CONTENT_LENGTH, "10")
            .to_request();
        let info = block_on(UrlEncoded::<_, Info>::new(&mut req));
        assert!(eq(info.err().unwrap(), UrlencodedError::ContentType));
    }

    #[test]
    fn test_urlencoded() {
        let mut req =
            TestRequest::with_header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(CONTENT_LENGTH, "11")
                .set_payload(Bytes::from_static(b"hello=world"))
                .to_request();

        let info = block_on(UrlEncoded::<_, Info>::new(&mut req)).unwrap();
        assert_eq!(
            info,
            Info {
                hello: "world".to_owned()
            }
        );

        let mut req = TestRequest::with_header(
            CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=utf-8",
        )
        .header(CONTENT_LENGTH, "11")
        .set_payload(Bytes::from_static(b"hello=world"))
        .to_request();

        let info = block_on(UrlEncoded::<_, Info>::new(&mut req)).unwrap();
        assert_eq!(
            info,
            Info {
                hello: "world".to_owned()
            }
        );
    }
}
