//! A purlin web service hosted by an fx application.
//!
//! ```text
//! cargo run -p purlin-on-fx          # then: curl localhost:3000/hello/fx
//! ```
//!
//! Two independent frameworks, one process, one lifecycle:
//!
//! - **purlin** owns the web side: `Greeter` (a service) and `HelloController`.
//! - **fx** owns the process side: a `Visits` counter with start/stop hooks, a graceful
//!   shutdown on Ctrl-C / SIGTERM, and the wiring of everything.
//! - **purlin-fx** is the only crate that knows both: it runs the purlin graph and the HTTP
//!   server as fx components.
#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use purlin::prelude::*;
use purlin_fx::{Hosted, hosted, serve};

// ---- the purlin side ---------------------------------------------------------------

#[injectable]
pub struct Greeter;

impl Greeter {
    fn greet(&self, name: &str) -> String {
        format!("hello, {name}")
    }
}

#[injectable]
pub struct HelloController {
    greeter: Arc<Greeter>,
}

#[controller("/hello")]
impl HelloController {
    #[get("/{name}")]
    async fn hello(&self, Path(name): Path<String>) -> String {
        self.greeter.greet(&name)
    }
}

#[module(providers = [Greeter], controllers = [HelloController])]
pub struct WebModule;

// ---- the fx side -------------------------------------------------------------------

/// Process-level state that has nothing to do with HTTP.
#[derive(Default)]
pub struct Visits(AtomicU32);

#[fx::component]
impl Visits {
    #[fx::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    #[fx::on_stop]
    fn report(&self) {
        println!("served {} visit(s)", self.0.load(Ordering::SeqCst));
    }
}

/// An fx function that reaches into the hosted purlin graph.
fn announce(host: Arc<Hosted>, _visits: Arc<Visits>) {
    println!("purlin modules: {:?}", host.modules());
}

pub fn app(address: &str) -> fx::App {
    fx::App::new()
        .component::<Visits>()
        .options(hosted::<WebModule>())
        .options(serve(address))
        .invoke(announce)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match app("127.0.0.1:3000").run().await {
        Ok(code) => std::process::ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;

    #[tokio::test]
    async fn the_purlin_controller_answers_while_fx_owns_the_process() {
        let mut app = app("127.0.0.1:0");
        app.start().await.expect("starts");

        let addr = app.get::<purlin_fx::HttpServer>().await.expect("server").local_addr();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /hello/fx HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("hello, fx"), "{response}");

        app.stop().await.expect("stops");
    }
}
