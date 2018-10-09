# Actix http [![Build Status](https://travis-ci.org/fafhrd91/actix-http.svg?branch=master)](https://travis-ci.org/fafhrd91/actix-http) [![Build status](https://ci.appveyor.com/api/projects/status/bwq6923pblqg55gk/branch/master?svg=true)](https://ci.appveyor.com/project/fafhrd91/actix-http/branch/master) [![codecov](https://codecov.io/gh/fafhrd91/actix-http/branch/master/graph/badge.svg)](https://codecov.io/gh/fafhrd91/actix-http) [![crates.io](https://meritbadge.herokuapp.com/actix-web)](https://crates.io/crates/actix-web) [![Join the chat at https://gitter.im/actix/actix](https://badges.gitter.im/actix/actix.svg)](https://gitter.im/actix/actix?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

Actix http

## Documentation & community resources

* [User Guide](https://actix.rs/docs/)
* [API Documentation (Development)](https://actix.rs/actix-http/actix_http/)
* [API Documentation (Releases)](https://actix.rs/api/actix-http/stable/actix_http/)
* [Chat on gitter](https://gitter.im/actix/actix)
* Cargo package: [actix-http](https://crates.io/crates/actix-web)
* Minimum supported Rust version: 1.26 or later

## Example

```rust
extern crate actix_http;
use actix_http::{h1, Response, ServiceConfig};

fn main() {
    Server::new()
        .bind("app", addr, move || {
            IntoFramed::new(|| h1::Codec::new(ServiceConfig::default())) // <- create h1 codec
                .and_then(TakeItem::new().map_err(|_| ()))      // <- read one request
                .and_then(|(req, framed): (_, Framed<_, _>)| {  // <- send response and close conn
                    framed
                        .send(h1::OutMessage::Response(Response::Ok().finish()))
                })
        })
        .run();
}
```

## License

This project is licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))
* MIT license ([LICENSE-MIT](LICENSE-MIT) or [http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))

at your option.

## Code of Conduct

Contribution to the actix-http crate is organized under the terms of the
Contributor Covenant, the maintainer of actix-http, @fafhrd91, promises to
intervene to uphold that code of conduct.
