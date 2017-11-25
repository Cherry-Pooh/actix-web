//! Error and Result module
use std::{fmt, result};
use std::str::Utf8Error;
use std::string::FromUtf8Error;
use std::io::Error as IoError;

#[cfg(actix_nightly)]
use std::error::Error as StdError;

use cookie;
use httparse;
use failure::Fail;
use http2::Error as Http2Error;
use http::{header, StatusCode, Error as HttpError};
use http_range::HttpRangeParseError;
use serde_json::error::Error as JsonError;

// re-exports
pub use cookie::{ParseError as CookieParseError};

use body::Body;
use httpresponse::HttpResponse;
use httpcodes::{HTTPBadRequest, HTTPMethodNotAllowed, HTTPExpectationFailed};

/// A specialized [`Result`](https://doc.rust-lang.org/std/result/enum.Result.html)
/// for actix web operations
///
/// This typedef is generally used to avoid writing out `actix_web::error::Error` directly and
/// is otherwise a direct mapping to `Result`.
pub type Result<T> = result::Result<T, Error>;

/// Actix web error
#[derive(Debug)]
pub struct Error {
    cause: Box<ErrorResponse>,
}

impl Error {

    /// Returns a reference to the underlying cause of this Error.
    // this should return &Fail but needs this https://github.com/rust-lang/rust/issues/5665
    pub fn cause(&self) -> &ErrorResponse {
        self.cause.as_ref()
    }
}

/// Error that can be converted to `HttpResponse`
pub trait ErrorResponse: Fail {

    /// Create response for error
    ///
    /// Internal server error is generated by default.
    fn error_response(&self) -> HttpResponse {
        HttpResponse::new(StatusCode::INTERNAL_SERVER_ERROR, Body::Empty)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.cause, f)
    }
}

/// `HttpResponse` for `Error`
impl From<Error> for HttpResponse {
    fn from(err: Error) -> Self {
        err.cause.error_response()
    }
}

/// `Error` for any error that implements `ErrorResponse`
impl<T: ErrorResponse> From<T> for Error {
    fn from(err: T) -> Error {
        Error { cause: Box::new(err) }
    }
}

/// Default error is `InternalServerError`
#[cfg(actix_nightly)]
default impl<T: StdError + Sync + Send + 'static> ErrorResponse for T {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::new(StatusCode::INTERNAL_SERVER_ERROR, Body::Empty)
    }
}

/// `InternalServerError` for `JsonError`
impl ErrorResponse for JsonError {}

/// Internal error
#[derive(Fail, Debug)]
#[fail(display="Unexpected task frame")]
pub struct UnexpectedTaskFrame;

impl ErrorResponse for UnexpectedTaskFrame {}

/// A set of errors that can occur during parsing HTTP streams
#[derive(Fail, Debug)]
pub enum ParseError {
    /// An invalid `Method`, such as `GE.T`.
    #[fail(display="Invalid Method specified")]
    Method,
    /// An invalid `Uri`, such as `exam ple.domain`.
    #[fail(display="Uri error")]
    Uri,
    /// An invalid `HttpVersion`, such as `HTP/1.1`
    #[fail(display="Invalid HTTP version specified")]
    Version,
    /// An invalid `Header`.
    #[fail(display="Invalid Header provided")]
    Header,
    /// A message head is too large to be reasonable.
    #[fail(display="Message head is too large")]
    TooLarge,
    /// A message reached EOF, but is not complete.
    #[fail(display="Message is incomplete")]
    Incomplete,
    /// An invalid `Status`, such as `1337 ELITE`.
    #[fail(display="Invalid Status provided")]
    Status,
    /// A timeout occurred waiting for an IO event.
    #[allow(dead_code)]
    #[fail(display="Timeout")]
    Timeout,
    /// An `io::Error` that occurred while trying to read or write to a network stream.
    #[fail(display="IO error: {}", _0)]
    Io(#[cause] IoError),
    /// Parsing a field as string failed
    #[fail(display="UTF8 error: {}", _0)]
    Utf8(#[cause] Utf8Error),
}

/// Return `BadRequest` for `ParseError`
impl ErrorResponse for ParseError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::new(StatusCode::BAD_REQUEST, Body::Empty)
    }
}

impl From<IoError> for ParseError {
    fn from(err: IoError) -> ParseError {
        ParseError::Io(err)
    }
}

impl From<Utf8Error> for ParseError {
    fn from(err: Utf8Error) -> ParseError {
        ParseError::Utf8(err)
    }
}

impl From<FromUtf8Error> for ParseError {
    fn from(err: FromUtf8Error) -> ParseError {
        ParseError::Utf8(err.utf8_error())
    }
}

impl From<httparse::Error> for ParseError {
    fn from(err: httparse::Error) -> ParseError {
        match err {
            httparse::Error::HeaderName |
            httparse::Error::HeaderValue |
            httparse::Error::NewLine |
            httparse::Error::Token => ParseError::Header,
            httparse::Error::Status => ParseError::Status,
            httparse::Error::TooManyHeaders => ParseError::TooLarge,
            httparse::Error::Version => ParseError::Version,
        }
    }
}

#[derive(Fail, Debug)]
/// A set of errors that can occur during payload parsing
pub enum PayloadError {
    /// A payload reached EOF, but is not complete.
    #[fail(display="A payload reached EOF, but is not complete.")]
    Incomplete,
    /// Content encoding stream corruption
    #[fail(display="Can not decode content-encoding.")]
    EncodingCorrupted,
    /// Parse error
    #[fail(display="{}", _0)]
    ParseError(#[cause] IoError),
    /// Http2 error
    #[fail(display="{}", _0)]
    Http2(#[cause] Http2Error),
}

impl From<IoError> for PayloadError {
    fn from(err: IoError) -> PayloadError {
        PayloadError::ParseError(err)
    }
}

/// Return `InternalServerError` for `HttpError`,
/// Response generation can return `HttpError`, so it is internal error
impl ErrorResponse for HttpError {}

/// Return `InternalServerError` for `io::Error`
impl ErrorResponse for IoError {}

/// Return `BadRequest` for `cookie::ParseError`
impl ErrorResponse for cookie::ParseError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::new(StatusCode::BAD_REQUEST, Body::Empty)
    }
}

/// Http range header parsing error
#[derive(Fail, PartialEq, Debug)]
pub enum HttpRangeError {
    /// Returned if range is invalid.
    #[fail(display="Range header is invalid")]
    InvalidRange,
    /// Returned if first-byte-pos of all of the byte-range-spec
    /// values is greater than the content size.
    /// See https://github.com/golang/go/commit/aa9b3d7
    #[fail(display="First-byte-pos of all of the byte-range-spec values is greater than the content size")]
    NoOverlap,
}

/// Return `BadRequest` for `HttpRangeError`
impl ErrorResponse for HttpRangeError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::new(
            StatusCode::BAD_REQUEST, Body::from("Invalid Range header provided"))
    }
}

impl From<HttpRangeParseError> for HttpRangeError {
    fn from(err: HttpRangeParseError) -> HttpRangeError {
        match err {
            HttpRangeParseError::InvalidRange => HttpRangeError::InvalidRange,
            HttpRangeParseError::NoOverlap => HttpRangeError::NoOverlap,
        }
    }
}

/// A set of errors that can occur during parsing multipart streams
#[derive(Fail, Debug)]
pub enum MultipartError {
    /// Content-Type header is not found
    #[fail(display="No Content-type header found")]
    NoContentType,
    /// Can not parse Content-Type header
    #[fail(display="Can not parse Content-Type header")]
    ParseContentType,
    /// Multipart boundary is not found
    #[fail(display="Multipart boundary is not found")]
    Boundary,
    /// Error during field parsing
    #[fail(display="{}", _0)]
    Parse(#[cause] ParseError),
    /// Payload error
    #[fail(display="{}", _0)]
    Payload(#[cause] PayloadError),
}

impl From<ParseError> for MultipartError {
    fn from(err: ParseError) -> MultipartError {
        MultipartError::Parse(err)
    }
}

impl From<PayloadError> for MultipartError {
    fn from(err: PayloadError) -> MultipartError {
        MultipartError::Payload(err)
    }
}

/// Return `BadRequest` for `MultipartError`
impl ErrorResponse for MultipartError {

    fn error_response(&self) -> HttpResponse {
        HttpResponse::new(StatusCode::BAD_REQUEST, Body::Empty)
    }
}

/// Error during handling `Expect` header
#[derive(Fail, PartialEq, Debug)]
pub enum ExpectError {
    /// Expect header value can not be converted to utf8
    #[fail(display="Expect header value can not be converted to utf8")]
    Encoding,
    /// Unknown expect value
    #[fail(display="Unknown expect value")]
    UnknownExpect,
}

impl ErrorResponse for ExpectError {

    fn error_response(&self) -> HttpResponse {
        HTTPExpectationFailed.with_body("Unknown Expect")
    }
}

/// Websocket handshake errors
#[derive(Fail, PartialEq, Debug)]
pub enum WsHandshakeError {
    /// Only get method is allowed
    #[fail(display="Method not allowed")]
    GetMethodRequired,
    /// Ugrade header if not set to websocket
    #[fail(display="Websocket upgrade is expected")]
    NoWebsocketUpgrade,
    /// Connection header is not set to upgrade
    #[fail(display="Connection upgrade is expected")]
    NoConnectionUpgrade,
    /// Websocket version header is not set
    #[fail(display="Websocket version header is required")]
    NoVersionHeader,
    /// Unsupported websockt version
    #[fail(display="Unsupported version")]
    UnsupportedVersion,
    /// Websocket key is not set or wrong
    #[fail(display="Unknown websocket key")]
    BadWebsocketKey,
}

impl ErrorResponse for WsHandshakeError {

    fn error_response(&self) -> HttpResponse {
        match *self {
            WsHandshakeError::GetMethodRequired => {
                HTTPMethodNotAllowed
                    .builder()
                    .header(header::ALLOW, "GET")
                    .finish()
                    .unwrap()
            }
            WsHandshakeError::NoWebsocketUpgrade =>
                HTTPBadRequest.with_reason("No WebSocket UPGRADE header found"),
            WsHandshakeError::NoConnectionUpgrade =>
                HTTPBadRequest.with_reason("No CONNECTION upgrade"),
            WsHandshakeError::NoVersionHeader =>
                HTTPBadRequest.with_reason("Websocket version header is required"),
            WsHandshakeError::UnsupportedVersion =>
                HTTPBadRequest.with_reason("Unsupported version"),
            WsHandshakeError::BadWebsocketKey =>
                HTTPBadRequest.with_reason("Handshake error")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::io;
    use httparse;
    use http::{StatusCode, Error as HttpError};
    use cookie::ParseError as CookieParseError;
    use super::*;

    #[test]
    #[cfg(actix_nightly)]
    fn test_nightly() {
        let resp: HttpResponse = IoError::new(io::ErrorKind::Other, "test").error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_into_response() {
        let resp: HttpResponse = ParseError::Incomplete.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp: HttpResponse = HttpRangeError::InvalidRange.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp: HttpResponse = CookieParseError::EmptyName.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp: HttpResponse = MultipartError::Boundary.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let err: HttpError = StatusCode::from_u16(10000).err().unwrap().into();
        let resp: HttpResponse = err.error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_cause() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let desc = orig.description().to_owned();
        let e = ParseError::Io(orig);
        assert_eq!(format!("{}", e.cause().unwrap()), desc);
    }

    #[test]
    fn test_error_cause() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let desc = orig.description().to_owned();
        let e = Error::from(orig);
        assert_eq!(format!("{}", e.cause()), desc);
    }

    #[test]
    fn test_error_display() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let desc = orig.description().to_owned();
        let e = Error::from(orig);
        assert_eq!(format!("{}", e), desc);
    }

    #[test]
    fn test_error_http_response() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let e = Error::from(orig);
        let resp: HttpResponse = e.into();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_range_error() {
        let e: HttpRangeError = HttpRangeParseError::InvalidRange.into();
        assert_eq!(e, HttpRangeError::InvalidRange);
        let e: HttpRangeError = HttpRangeParseError::NoOverlap.into();
        assert_eq!(e, HttpRangeError::NoOverlap);
    }

    #[test]
    fn test_expect_error() {
        let resp: HttpResponse = ExpectError::Encoding.error_response();
        assert_eq!(resp.status(), StatusCode::EXPECTATION_FAILED);
        let resp: HttpResponse = ExpectError::UnknownExpect.error_response();
        assert_eq!(resp.status(), StatusCode::EXPECTATION_FAILED);
    }

    #[test]
    fn test_wserror_http_response() {
        let resp: HttpResponse = WsHandshakeError::GetMethodRequired.error_response();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let resp: HttpResponse = WsHandshakeError::NoWebsocketUpgrade.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp: HttpResponse = WsHandshakeError::NoConnectionUpgrade.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp: HttpResponse = WsHandshakeError::NoVersionHeader.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp: HttpResponse = WsHandshakeError::UnsupportedVersion.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp: HttpResponse = WsHandshakeError::BadWebsocketKey.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    macro_rules! from {
        ($from:expr => $error:pat) => {
            match ParseError::from($from) {
                e @ $error => {
                    assert!(format!("{}", e).len() >= 5);
                } ,
                e => panic!("{:?}", e)
            }
        }
    }

    macro_rules! from_and_cause {
        ($from:expr => $error:pat) => {
            match ParseError::from($from) {
                e @ $error => {
                    let desc = format!("{}", e.cause().unwrap());
                    assert_eq!(desc, $from.description().to_owned());
                },
                _ => panic!("{:?}", $from)
            }
        }
    }

    #[test]
    fn test_from() {
        from_and_cause!(io::Error::new(io::ErrorKind::Other, "other") => ParseError::Io(..));

        from!(httparse::Error::HeaderName => ParseError::Header);
        from!(httparse::Error::HeaderName => ParseError::Header);
        from!(httparse::Error::HeaderValue => ParseError::Header);
        from!(httparse::Error::NewLine => ParseError::Header);
        from!(httparse::Error::Status => ParseError::Status);
        from!(httparse::Error::Token => ParseError::Header);
        from!(httparse::Error::TooManyHeaders => ParseError::TooLarge);
        from!(httparse::Error::Version => ParseError::Version);
    }
}
