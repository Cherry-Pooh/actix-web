//! Various helpers for Actix applications to use during testing.
use std::sync::mpsc;
use std::{net, thread, time};

use actix_codec::{AsyncRead, AsyncWrite, Framed};
use actix_http::body::MessageBody;
use actix_http::client::{
    ClientRequest, ClientRequestBuilder, ClientResponse, ConnectError, Connection,
    Connector, SendRequestError,
};
use actix_http::{http::Uri, ws};
use actix_rt::{Runtime, System};
use actix_server::{Server, StreamServiceFactory};
use actix_service::Service;
use futures::future::{lazy, Future};
use http::Method;
use net2::TcpBuilder;

/// The `TestServer` type.
///
/// `TestServer` is very simple test server that simplify process of writing
/// integration tests cases for actix web applications.
///
/// # Examples
///
/// ```rust
/// # extern crate actix_web;
/// # use actix_web::*;
/// #
/// # fn my_handler(req: &HttpRequest) -> HttpResponse {
/// #     HttpResponse::Ok().into()
/// # }
/// #
/// # fn main() {
/// use actix_web::test::TestServer;
///
/// let mut srv = TestServer::new(|app| app.handler(my_handler));
///
/// let req = srv.get().finish().unwrap();
/// let response = srv.execute(req.send()).unwrap();
/// assert!(response.status().is_success());
/// # }
/// ```
pub struct TestServer;

/// Test server controller
pub struct TestServerRuntime {
    addr: net::SocketAddr,
    rt: Runtime,
}

impl TestServer {
    /// Start new test server with application factory
    pub fn new<F: StreamServiceFactory>(factory: F) -> TestServerRuntime {
        let (tx, rx) = mpsc::channel();

        // run server in separate thread
        thread::spawn(move || {
            let sys = System::new("actix-test-server");
            let tcp = net::TcpListener::bind("127.0.0.1:0").unwrap();
            let local_addr = tcp.local_addr().unwrap();

            Server::build()
                .listen("test", tcp, factory)?
                .workers(1)
                .disable_signals()
                .start();

            tx.send((System::current(), local_addr)).unwrap();
            sys.run()
        });

        let (system, addr) = rx.recv().unwrap();
        System::set_current(system);
        TestServerRuntime {
            addr,
            rt: Runtime::new().unwrap(),
        }
    }

    /// Get firat available unused address
    pub fn unused_addr() -> net::SocketAddr {
        let addr: net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let socket = TcpBuilder::new_v4().unwrap();
        socket.bind(&addr).unwrap();
        socket.reuse_address(true).unwrap();
        let tcp = socket.to_tcp_listener().unwrap();
        tcp.local_addr().unwrap()
    }
}

impl TestServerRuntime {
    /// Execute future on current core
    pub fn block_on<F, I, E>(&mut self, fut: F) -> Result<I, E>
    where
        F: Future<Item = I, Error = E>,
    {
        self.rt.block_on(fut)
    }

    /// Execute function on current core
    pub fn execute<F, R>(&mut self, fut: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.rt.block_on(lazy(|| Ok::<_, ()>(fut()))).unwrap()
    }

    /// Construct test server url
    pub fn addr(&self) -> net::SocketAddr {
        self.addr
    }

    /// Construct test server url
    pub fn url(&self, uri: &str) -> String {
        if uri.starts_with('/') {
            format!("http://127.0.0.1:{}{}", self.addr.port(), uri)
        } else {
            format!("http://127.0.0.1:{}/{}", self.addr.port(), uri)
        }
    }

    /// Construct test https server url
    pub fn surl(&self, uri: &str) -> String {
        if uri.starts_with('/') {
            format!("https://127.0.0.1:{}{}", self.addr.port(), uri)
        } else {
            format!("https://127.0.0.1:{}/{}", self.addr.port(), uri)
        }
    }

    /// Create `GET` request
    pub fn get(&self) -> ClientRequestBuilder {
        ClientRequest::get(self.url("/").as_str())
    }

    /// Create https `GET` request
    pub fn sget(&self) -> ClientRequestBuilder {
        ClientRequest::get(self.surl("/").as_str())
    }

    /// Create `POST` request
    pub fn post(&self) -> ClientRequestBuilder {
        ClientRequest::post(self.url("/").as_str())
    }

    /// Create `HEAD` request
    pub fn head(&self) -> ClientRequestBuilder {
        ClientRequest::head(self.url("/").as_str())
    }

    /// Connect to test http server
    pub fn client(&self, meth: Method, path: &str) -> ClientRequestBuilder {
        ClientRequest::build()
            .method(meth)
            .uri(self.url(path).as_str())
            .take()
    }

    fn new_connector(
    ) -> impl Service<Request = Uri, Response = impl Connection, Error = ConnectError> + Clone
    {
        #[cfg(feature = "ssl")]
        {
            use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};

            let mut builder = SslConnector::builder(SslMethod::tls()).unwrap();
            builder.set_verify(SslVerifyMode::NONE);
            let _ = builder
                .set_alpn_protos(b"\x02h2\x08http/1.1")
                .map_err(|e| log::error!("Can not set alpn protocol: {:?}", e));
            Connector::new()
                .timeout(time::Duration::from_millis(500))
                .ssl(builder.build())
                .service()
        }
        #[cfg(not(feature = "ssl"))]
        {
            Connector::new()
                .timeout(time::Duration::from_millis(500))
                .service()
        }
    }

    /// Http connector
    pub fn connector(
        &mut self,
    ) -> impl Service<Request = Uri, Response = impl Connection, Error = ConnectError> + Clone
    {
        self.execute(|| TestServerRuntime::new_connector())
    }

    /// Stop http server
    fn stop(&mut self) {
        System::current().stop();
    }
}

impl TestServerRuntime {
    /// Connect to websocket server at a given path
    pub fn ws_at(
        &mut self,
        path: &str,
    ) -> Result<Framed<impl AsyncRead + AsyncWrite, ws::Codec>, ws::ClientError> {
        let url = self.url(path);
        self.rt
            .block_on(lazy(|| ws::Client::default().call(ws::Connect::new(url))))
    }

    /// Connect to a websocket server
    pub fn ws(
        &mut self,
    ) -> Result<Framed<impl AsyncRead + AsyncWrite, ws::Codec>, ws::ClientError> {
        self.ws_at("/")
    }

    /// Send request and read response message
    pub fn send_request<B: MessageBody + 'static>(
        &mut self,
        req: ClientRequest<B>,
    ) -> Result<ClientResponse, SendRequestError> {
        let mut conn = self.connector();
        self.rt.block_on(req.send(&mut conn))
    }
}

impl Drop for TestServerRuntime {
    fn drop(&mut self) {
        self.stop()
    }
}
