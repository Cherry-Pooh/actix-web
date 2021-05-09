//! Error and Result module

use std::{
    cell::RefCell,
    error::Error as StdError,
    fmt,
    io::{self, Write as _},
    str::Utf8Error,
    string::FromUtf8Error,
};

use bytes::BytesMut;
use derive_more::{Display, Error, From};
use http::{header, uri::InvalidUri, StatusCode};
use serde::de::value::Error as DeError;

use crate::{body::Body, helpers::Writer, Response, ResponseBuilder};

pub use http::Error as HttpError;

/// General purpose actix web error.
///
/// An actix web error is used to carry errors from `std::error`
/// through actix in a convenient way.  It can be created through
/// converting errors with `into()`.
///
/// Whenever it is created from an external object a response error is created
/// for it that can be used to create an HTTP response from it this means that
/// if you have access to an actix `Error` you can always get a
/// `ResponseError` reference from it.
pub struct Error {
    cause: Box<dyn ResponseError>,
}

impl Error {
    /// Returns the reference to the underlying `ResponseError`.
    pub fn as_response_error(&self) -> &dyn ResponseError {
        self.cause.as_ref()
    }

    /// Similar to `as_response_error` but downcasts.
    pub fn as_error<T: ResponseError + 'static>(&self) -> Option<&T> {
        <dyn ResponseError>::downcast_ref(self.cause.as_ref())
    }
}

/// Errors that can generate responses.
pub trait ResponseError: fmt::Debug + fmt::Display {
    /// Returns appropriate status code for error.
    ///
    /// A 500 Internal Server Error is used by default. If [error_response](Self::error_response) is
    /// also implemented and does not call `self.status_code()`, then this will not be used.
    fn status_code(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    /// Creates full response for error.
    ///
    /// By default, the generated response uses a 500 Internal Server Error status code, a
    /// `Content-Type` of `text/plain`, and the body is set to `Self`'s `Display` impl.
    fn error_response(&self) -> Response<Body> {
        let mut resp = Response::new(self.status_code());
        let mut buf = BytesMut::new();
        let _ = write!(Writer(&mut buf), "{}", self);
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        resp.set_body(Body::from(buf))
    }

    downcast_get_type_id!();
}

downcast!(ResponseError);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, f)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &self.cause)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl From<()> for Error {
    fn from(_: ()) -> Self {
        Error::from(UnitError)
    }
}

impl From<std::convert::Infallible> for Error {
    fn from(_: std::convert::Infallible) -> Self {
        // hint that an error that will never happen
        unreachable!()
    }
}

/// Convert `Error` to a `Response` instance
impl From<Error> for Response<Body> {
    fn from(err: Error) -> Self {
        Response::from_error(err)
    }
}

/// `Error` for any error that implements `ResponseError`
impl<T: ResponseError + 'static> From<T> for Error {
    fn from(err: T) -> Error {
        Error {
            cause: Box::new(err),
        }
    }
}

/// Convert Response to a Error
impl From<Response<Body>> for Error {
    fn from(res: Response<Body>) -> Error {
        InternalError::from_response("", res).into()
    }
}

/// Convert ResponseBuilder to a Error
impl From<ResponseBuilder> for Error {
    fn from(mut res: ResponseBuilder) -> Error {
        InternalError::from_response("", res.finish()).into()
    }
}

#[derive(Debug, Display, Error)]
#[display(fmt = "Unknown Error")]
struct UnitError;

impl ResponseError for Box<dyn StdError + 'static> {}

/// Returns [`StatusCode::INTERNAL_SERVER_ERROR`] for [`UnitError`].
impl ResponseError for UnitError {}

/// Returns [`StatusCode::INTERNAL_SERVER_ERROR`] for [`actix_tls::accept::openssl::SslError`].
#[cfg(feature = "openssl")]
impl ResponseError for actix_tls::accept::openssl::SslError {}

/// Returns [`StatusCode::BAD_REQUEST`] for [`DeError`].
impl ResponseError for DeError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// Returns [`StatusCode::BAD_REQUEST`] for [`Utf8Error`].
impl ResponseError for Utf8Error {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// Returns [`StatusCode::INTERNAL_SERVER_ERROR`] for [`HttpError`].
impl ResponseError for HttpError {}

/// Inspects the underlying [`io::ErrorKind`] and returns an appropriate status code.
///
/// If the error is [`io::ErrorKind::NotFound`], [`StatusCode::NOT_FOUND`] is returned. If the
/// error is [`io::ErrorKind::PermissionDenied`], [`StatusCode::FORBIDDEN`] is returned. Otherwise,
/// [`StatusCode::INTERNAL_SERVER_ERROR`] is returned.
impl ResponseError for io::Error {
    fn status_code(&self) -> StatusCode {
        match self.kind() {
            io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Returns [`StatusCode::BAD_REQUEST`] for [`header::InvalidHeaderValue`].
impl ResponseError for header::InvalidHeaderValue {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// A set of errors that can occur during parsing HTTP streams.
#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum ParseError {
    /// An invalid `Method`, such as `GE.T`.
    #[display(fmt = "Invalid Method specified")]
    Method,

    /// An invalid `Uri`, such as `exam ple.domain`.
    #[display(fmt = "Uri error: {}", _0)]
    Uri(InvalidUri),

    /// An invalid `HttpVersion`, such as `HTP/1.1`
    #[display(fmt = "Invalid HTTP version specified")]
    Version,

    /// An invalid `Header`.
    #[display(fmt = "Invalid Header provided")]
    Header,

    /// A message head is too large to be reasonable.
    #[display(fmt = "Message head is too large")]
    TooLarge,

    /// A message reached EOF, but is not complete.
    #[display(fmt = "Message is incomplete")]
    Incomplete,

    /// An invalid `Status`, such as `1337 ELITE`.
    #[display(fmt = "Invalid Status provided")]
    Status,

    /// A timeout occurred waiting for an IO event.
    #[allow(dead_code)]
    #[display(fmt = "Timeout")]
    Timeout,

    /// An `io::Error` that occurred while trying to read or write to a network stream.
    #[display(fmt = "IO error: {}", _0)]
    Io(io::Error),

    /// Parsing a field as string failed
    #[display(fmt = "UTF8 error: {}", _0)]
    Utf8(Utf8Error),
}

/// Return `BadRequest` for `ParseError`
impl ResponseError for ParseError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

impl From<io::Error> for ParseError {
    fn from(err: io::Error) -> ParseError {
        ParseError::Io(err)
    }
}

impl From<InvalidUri> for ParseError {
    fn from(err: InvalidUri) -> ParseError {
        ParseError::Uri(err)
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
            httparse::Error::HeaderName
            | httparse::Error::HeaderValue
            | httparse::Error::NewLine
            | httparse::Error::Token => ParseError::Header,
            httparse::Error::Status => ParseError::Status,
            httparse::Error::TooManyHeaders => ParseError::TooLarge,
            httparse::Error::Version => ParseError::Version,
        }
    }
}

/// A set of errors that can occur running blocking tasks in thread pool.
#[derive(Debug, Display, Error)]
#[display(fmt = "Blocking thread pool is gone")]
pub struct BlockingError;

/// `InternalServerError` for `BlockingError`
impl ResponseError for BlockingError {}

/// A set of errors that can occur during payload parsing.
#[derive(Debug, Display)]
#[non_exhaustive]
pub enum PayloadError {
    /// A payload reached EOF, but is not complete.
    #[display(
        fmt = "A payload reached EOF, but is not complete. Inner error: {:?}",
        _0
    )]
    Incomplete(Option<io::Error>),

    /// Content encoding stream corruption.
    #[display(fmt = "Can not decode content-encoding.")]
    EncodingCorrupted,

    /// Payload reached size limit.
    #[display(fmt = "Payload reached size limit.")]
    Overflow,

    /// Payload length is unknown.
    #[display(fmt = "Payload length is unknown.")]
    UnknownLength,

    /// HTTP/2 payload error.
    #[display(fmt = "{}", _0)]
    Http2Payload(h2::Error),

    /// Generic I/O error.
    #[display(fmt = "{}", _0)]
    Io(io::Error),
}

impl std::error::Error for PayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PayloadError::Incomplete(None) => None,
            PayloadError::Incomplete(Some(err)) => Some(err as &dyn std::error::Error),
            PayloadError::EncodingCorrupted => None,
            PayloadError::Overflow => None,
            PayloadError::UnknownLength => None,
            PayloadError::Http2Payload(err) => Some(err as &dyn std::error::Error),
            PayloadError::Io(err) => Some(err as &dyn std::error::Error),
        }
    }
}

impl From<h2::Error> for PayloadError {
    fn from(err: h2::Error) -> Self {
        PayloadError::Http2Payload(err)
    }
}

impl From<Option<io::Error>> for PayloadError {
    fn from(err: Option<io::Error>) -> Self {
        PayloadError::Incomplete(err)
    }
}

impl From<io::Error> for PayloadError {
    fn from(err: io::Error) -> Self {
        PayloadError::Incomplete(Some(err))
    }
}

impl From<BlockingError> for PayloadError {
    fn from(_: BlockingError) -> Self {
        PayloadError::Io(io::Error::new(
            io::ErrorKind::Other,
            "Operation is canceled",
        ))
    }
}

/// `PayloadError` returns two possible results:
///
/// - `Overflow` returns `PayloadTooLarge`
/// - Other errors returns `BadRequest`
impl ResponseError for PayloadError {
    fn status_code(&self) -> StatusCode {
        match *self {
            PayloadError::Overflow => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

/// A set of errors that can occur during dispatching HTTP requests.
#[derive(Debug, Display, Error, From)]
#[non_exhaustive]
pub enum DispatchError {
    /// Service error
    Service(Error),

    /// Upgrade service error
    Upgrade,

    /// An `io::Error` that occurred while trying to read or write to a network
    /// stream.
    #[display(fmt = "IO error: {}", _0)]
    Io(io::Error),

    /// Http request parse error.
    #[display(fmt = "Parse error: {}", _0)]
    Parse(ParseError),

    /// Http/2 error
    #[display(fmt = "{}", _0)]
    H2(h2::Error),

    /// The first request did not complete within the specified timeout.
    #[display(fmt = "The first request did not complete within the specified timeout")]
    SlowRequestTimeout,

    /// Disconnect timeout. Makes sense for ssl streams.
    #[display(fmt = "Connection shutdown timeout")]
    DisconnectTimeout,

    /// Payload is not consumed
    #[display(fmt = "Task is completed but request's payload is not consumed")]
    PayloadIsNotConsumed,

    /// Malformed request
    #[display(fmt = "Malformed request")]
    MalformedRequest,

    /// Internal error
    #[display(fmt = "Internal error")]
    InternalError,

    /// Unknown error
    #[display(fmt = "Unknown error")]
    Unknown,
}

/// A set of error that can occur during parsing content type.
#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum ContentTypeError {
    /// Can not parse content type
    #[display(fmt = "Can not parse content type")]
    ParseError,

    /// Unknown content encoding
    #[display(fmt = "Unknown content encoding")]
    UnknownEncoding,
}

#[cfg(test)]
mod content_type_test_impls {
    use super::*;

    impl std::cmp::PartialEq for ContentTypeError {
        fn eq(&self, other: &Self) -> bool {
            match self {
                Self::ParseError => matches!(other, ContentTypeError::ParseError),
                Self::UnknownEncoding => {
                    matches!(other, ContentTypeError::UnknownEncoding)
                }
            }
        }
    }
}

/// Return `BadRequest` for `ContentTypeError`
impl ResponseError for ContentTypeError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// Helper type that can wrap any error and generate custom response.
///
/// In following example any `io::Error` will be converted into "BAD REQUEST"
/// response as opposite to *INTERNAL SERVER ERROR* which is defined by
/// default.
///
/// ```
/// # use std::io;
/// # use actix_http::{error, Request};
/// fn index(req: Request) -> Result<&'static str, actix_http::Error> {
///     Err(error::ErrorBadRequest(io::Error::new(io::ErrorKind::Other, "error")))
/// }
/// ```
pub struct InternalError<T> {
    cause: T,
    status: InternalErrorType,
}

enum InternalErrorType {
    Status(StatusCode),
    Response(RefCell<Option<Response<Body>>>),
}

impl<T> InternalError<T> {
    /// Create `InternalError` instance
    pub fn new(cause: T, status: StatusCode) -> Self {
        InternalError {
            cause,
            status: InternalErrorType::Status(status),
        }
    }

    /// Create `InternalError` with predefined `Response`.
    pub fn from_response(cause: T, response: Response<Body>) -> Self {
        InternalError {
            cause,
            status: InternalErrorType::Response(RefCell::new(Some(response))),
        }
    }
}

impl<T> fmt::Debug for InternalError<T>
where
    T: fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.cause, f)
    }
}

impl<T> fmt::Display for InternalError<T>
where
    T: fmt::Display + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, f)
    }
}

impl<T> ResponseError for InternalError<T>
where
    T: fmt::Debug + fmt::Display + 'static,
{
    fn status_code(&self) -> StatusCode {
        match self.status {
            InternalErrorType::Status(st) => st,
            InternalErrorType::Response(ref resp) => {
                if let Some(resp) = resp.borrow().as_ref() {
                    resp.head().status
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
    }

    fn error_response(&self) -> Response<Body> {
        match self.status {
            InternalErrorType::Status(st) => {
                let mut res = Response::new(st);
                let mut buf = BytesMut::new();
                let _ = write!(Writer(&mut buf), "{}", self);
                res.headers_mut().insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("text/plain; charset=utf-8"),
                );
                res.set_body(Body::from(buf))
            }
            InternalErrorType::Response(ref resp) => {
                if let Some(resp) = resp.borrow_mut().take() {
                    resp
                } else {
                    Response::new(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
    }
}

macro_rules! error_helper {
    ($name:ident, $status:ident) => {
        paste::paste! {
            #[doc = "Helper function that wraps any error and generates a `" $status "` response."]
            #[allow(non_snake_case)]
            pub fn $name<T>(err: T) -> Error
            where
            T: fmt::Debug + fmt::Display + 'static,
            {
                InternalError::new(err, StatusCode::$status).into()
            }
        }
    }
}

error_helper!(ErrorBadRequest, BAD_REQUEST);
error_helper!(ErrorUnauthorized, UNAUTHORIZED);
error_helper!(ErrorPaymentRequired, PAYMENT_REQUIRED);
error_helper!(ErrorForbidden, FORBIDDEN);
error_helper!(ErrorNotFound, NOT_FOUND);
error_helper!(ErrorMethodNotAllowed, METHOD_NOT_ALLOWED);
error_helper!(ErrorNotAcceptable, NOT_ACCEPTABLE);
error_helper!(
    ErrorProxyAuthenticationRequired,
    PROXY_AUTHENTICATION_REQUIRED
);
error_helper!(ErrorRequestTimeout, REQUEST_TIMEOUT);
error_helper!(ErrorConflict, CONFLICT);
error_helper!(ErrorGone, GONE);
error_helper!(ErrorLengthRequired, LENGTH_REQUIRED);
error_helper!(ErrorPayloadTooLarge, PAYLOAD_TOO_LARGE);
error_helper!(ErrorUriTooLong, URI_TOO_LONG);
error_helper!(ErrorUnsupportedMediaType, UNSUPPORTED_MEDIA_TYPE);
error_helper!(ErrorRangeNotSatisfiable, RANGE_NOT_SATISFIABLE);
error_helper!(ErrorImATeapot, IM_A_TEAPOT);
error_helper!(ErrorMisdirectedRequest, MISDIRECTED_REQUEST);
error_helper!(ErrorUnprocessableEntity, UNPROCESSABLE_ENTITY);
error_helper!(ErrorLocked, LOCKED);
error_helper!(ErrorFailedDependency, FAILED_DEPENDENCY);
error_helper!(ErrorUpgradeRequired, UPGRADE_REQUIRED);
error_helper!(ErrorPreconditionFailed, PRECONDITION_FAILED);
error_helper!(ErrorPreconditionRequired, PRECONDITION_REQUIRED);
error_helper!(ErrorTooManyRequests, TOO_MANY_REQUESTS);
error_helper!(
    ErrorRequestHeaderFieldsTooLarge,
    REQUEST_HEADER_FIELDS_TOO_LARGE
);
error_helper!(
    ErrorUnavailableForLegalReasons,
    UNAVAILABLE_FOR_LEGAL_REASONS
);
error_helper!(ErrorExpectationFailed, EXPECTATION_FAILED);
error_helper!(ErrorInternalServerError, INTERNAL_SERVER_ERROR);
error_helper!(ErrorNotImplemented, NOT_IMPLEMENTED);
error_helper!(ErrorBadGateway, BAD_GATEWAY);
error_helper!(ErrorServiceUnavailable, SERVICE_UNAVAILABLE);
error_helper!(ErrorGatewayTimeout, GATEWAY_TIMEOUT);
error_helper!(ErrorHttpVersionNotSupported, HTTP_VERSION_NOT_SUPPORTED);
error_helper!(ErrorVariantAlsoNegotiates, VARIANT_ALSO_NEGOTIATES);
error_helper!(ErrorInsufficientStorage, INSUFFICIENT_STORAGE);
error_helper!(ErrorLoopDetected, LOOP_DETECTED);
error_helper!(ErrorNotExtended, NOT_EXTENDED);
error_helper!(
    ErrorNetworkAuthenticationRequired,
    NETWORK_AUTHENTICATION_REQUIRED
);

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Error as HttpError, StatusCode};
    use std::io;

    #[test]
    fn test_into_response() {
        let resp: Response<Body> = ParseError::Incomplete.error_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let err: HttpError = StatusCode::from_u16(10000).err().unwrap().into();
        let resp: Response<Body> = err.error_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_as_response() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let e: Error = ParseError::Io(orig).into();
        assert_eq!(format!("{}", e.as_response_error()), "IO error: other");
    }

    #[test]
    fn test_error_cause() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let desc = orig.to_string();
        let e = Error::from(orig);
        assert_eq!(format!("{}", e.as_response_error()), desc);
    }

    #[test]
    fn test_error_display() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let desc = orig.to_string();
        let e = Error::from(orig);
        assert_eq!(format!("{}", e), desc);
    }

    #[test]
    fn test_error_http_response() {
        let orig = io::Error::new(io::ErrorKind::Other, "other");
        let e = Error::from(orig);
        let resp: Response<Body> = e.into();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_payload_error() {
        let err: PayloadError =
            io::Error::new(io::ErrorKind::Other, "ParseError").into();
        assert!(err.to_string().contains("ParseError"));

        let err = PayloadError::Incomplete(None);
        assert_eq!(
            err.to_string(),
            "A payload reached EOF, but is not complete. Inner error: None"
        );
    }

    macro_rules! from {
        ($from:expr => $error:pat) => {
            match ParseError::from($from) {
                err @ $error => {
                    assert!(err.to_string().len() >= 5);
                }
                err => unreachable!("{:?}", err),
            }
        };
    }

    macro_rules! from_and_cause {
        ($from:expr => $error:pat) => {
            match ParseError::from($from) {
                e @ $error => {
                    let desc = format!("{}", e);
                    assert_eq!(desc, format!("IO error: {}", $from));
                }
                _ => unreachable!("{:?}", $from),
            }
        };
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

    #[test]
    fn test_internal_error() {
        let err = InternalError::from_response(ParseError::Method, Response::ok());
        let resp: Response<Body> = err.error_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_error_casting() {
        let err = PayloadError::Overflow;
        let resp_err: &dyn ResponseError = &err;
        let err = resp_err.downcast_ref::<PayloadError>().unwrap();
        assert_eq!(err.to_string(), "Payload reached size limit.");
        let not_err = resp_err.downcast_ref::<ContentTypeError>();
        assert!(not_err.is_none());
    }

    #[test]
    fn test_error_helpers() {
        let res: Response<Body> = ErrorBadRequest("err").into();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let res: Response<Body> = ErrorUnauthorized("err").into();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let res: Response<Body> = ErrorPaymentRequired("err").into();
        assert_eq!(res.status(), StatusCode::PAYMENT_REQUIRED);

        let res: Response<Body> = ErrorForbidden("err").into();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let res: Response<Body> = ErrorNotFound("err").into();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let res: Response<Body> = ErrorMethodNotAllowed("err").into();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

        let res: Response<Body> = ErrorNotAcceptable("err").into();
        assert_eq!(res.status(), StatusCode::NOT_ACCEPTABLE);

        let res: Response<Body> = ErrorProxyAuthenticationRequired("err").into();
        assert_eq!(res.status(), StatusCode::PROXY_AUTHENTICATION_REQUIRED);

        let res: Response<Body> = ErrorRequestTimeout("err").into();
        assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);

        let res: Response<Body> = ErrorConflict("err").into();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        let res: Response<Body> = ErrorGone("err").into();
        assert_eq!(res.status(), StatusCode::GONE);

        let res: Response<Body> = ErrorLengthRequired("err").into();
        assert_eq!(res.status(), StatusCode::LENGTH_REQUIRED);

        let res: Response<Body> = ErrorPreconditionFailed("err").into();
        assert_eq!(res.status(), StatusCode::PRECONDITION_FAILED);

        let res: Response<Body> = ErrorPayloadTooLarge("err").into();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let res: Response<Body> = ErrorUriTooLong("err").into();
        assert_eq!(res.status(), StatusCode::URI_TOO_LONG);

        let res: Response<Body> = ErrorUnsupportedMediaType("err").into();
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let res: Response<Body> = ErrorRangeNotSatisfiable("err").into();
        assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let res: Response<Body> = ErrorExpectationFailed("err").into();
        assert_eq!(res.status(), StatusCode::EXPECTATION_FAILED);

        let res: Response<Body> = ErrorImATeapot("err").into();
        assert_eq!(res.status(), StatusCode::IM_A_TEAPOT);

        let res: Response<Body> = ErrorMisdirectedRequest("err").into();
        assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST);

        let res: Response<Body> = ErrorUnprocessableEntity("err").into();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let res: Response<Body> = ErrorLocked("err").into();
        assert_eq!(res.status(), StatusCode::LOCKED);

        let res: Response<Body> = ErrorFailedDependency("err").into();
        assert_eq!(res.status(), StatusCode::FAILED_DEPENDENCY);

        let res: Response<Body> = ErrorUpgradeRequired("err").into();
        assert_eq!(res.status(), StatusCode::UPGRADE_REQUIRED);

        let res: Response<Body> = ErrorPreconditionRequired("err").into();
        assert_eq!(res.status(), StatusCode::PRECONDITION_REQUIRED);

        let res: Response<Body> = ErrorTooManyRequests("err").into();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);

        let res: Response<Body> = ErrorRequestHeaderFieldsTooLarge("err").into();
        assert_eq!(res.status(), StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);

        let res: Response<Body> = ErrorUnavailableForLegalReasons("err").into();
        assert_eq!(res.status(), StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS);

        let res: Response<Body> = ErrorInternalServerError("err").into();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let res: Response<Body> = ErrorNotImplemented("err").into();
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);

        let res: Response<Body> = ErrorBadGateway("err").into();
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);

        let res: Response<Body> = ErrorServiceUnavailable("err").into();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

        let res: Response<Body> = ErrorGatewayTimeout("err").into();
        assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);

        let res: Response<Body> = ErrorHttpVersionNotSupported("err").into();
        assert_eq!(res.status(), StatusCode::HTTP_VERSION_NOT_SUPPORTED);

        let res: Response<Body> = ErrorVariantAlsoNegotiates("err").into();
        assert_eq!(res.status(), StatusCode::VARIANT_ALSO_NEGOTIATES);

        let res: Response<Body> = ErrorInsufficientStorage("err").into();
        assert_eq!(res.status(), StatusCode::INSUFFICIENT_STORAGE);

        let res: Response<Body> = ErrorLoopDetected("err").into();
        assert_eq!(res.status(), StatusCode::LOOP_DETECTED);

        let res: Response<Body> = ErrorNotExtended("err").into();
        assert_eq!(res.status(), StatusCode::NOT_EXTENDED);

        let res: Response<Body> = ErrorNetworkAuthenticationRequired("err").into();
        assert_eq!(res.status(), StatusCode::NETWORK_AUTHENTICATION_REQUIRED);
    }
}
