use structopt::StructOpt;
use tracing::info;

use std::net::SocketAddr;
use std::time::Duration;

mod client;

use client::*;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "gossip",
    about = "Simple distributed application for gossiping random info"
)]
struct Options {
    /// Addresses to connect to
    #[structopt(short)]
    connect: Vec<SocketAddr>,

    /// Time in seconds for transporting random data
    #[structopt(short)]
    time: u64,

    /// Address to listen for connections
    #[structopt(name = "ADDR")]
    bind: SocketAddr,
}

fn setup_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::{fmt, Registry};

    let fmt = fmt::layer()
        .with_level(true)
        .with_timer(fmt::time::SystemTime)
        .with_thread_names(true);

    let subscriber = Registry::default().with(fmt);
    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");
}

#[tokio::main]
async fn main() {
    let Options {
        connect,
        time,
        bind,
    } = Options::from_args();
    setup_tracing();

    let mut client = Client::new(bind).unwrap();

    for c in connect {
        client.connect(c).await.unwrap();
    }

    let time = Duration::from_secs(time);

    loop {
        let data = {
            let len = rand::random::<usize>() % 5;
            (0..len).map(|_| rand::random()).collect::<Vec<_>>()
        };

        client.send_data(&*data).await;
        tokio::time::sleep(time).await;
    }
}
