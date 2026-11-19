//! `cargo run -p fx-axum`, then `curl -X POST localhost:3000/greetings/Ada`.

use fx_axum::web::server::ServerConfig;

#[fx::main]
async fn main() -> fx::Result<()> {
    // Ctrl-C triggers the app's graceful stop: the server drains, then hooks unwind.
    let app = fx_axum::app(ServerConfig::default());
    let stop = app.shutdowner();
    fx::tokio::spawn(async move {
        let _ = fx::tokio::signal::ctrl_c().await;
        stop.shutdown();
    });
    let code = app.run().await?;
    std::process::exit(code);
}
