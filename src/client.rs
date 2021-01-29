use displaydoc::Display;
use serde::{de, ser, Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, instrument};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpSocket, TcpStream};
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::RwLock;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

mod expiring_set;
mod id;

use expiring_set::ExpiringSet;
pub use id::*;

/// Result type for this crate
pub type Result<T> = std::result::Result<T, Error>;

/// Sink error typefor gossip client
#[derive(Display, Error, Debug)]
pub enum Error {
    /// IO Error: {0:?}
    IOError(#[from] std::io::Error),
    /// Cbor serde error: {0:?}
    CborError(#[from] serde_cbor::Error),
    /// Recieved message without propper signature
    SignatureError,

    /// Failed to send event: {0:?}
    SendEventError(#[from] mpsc::error::SendError<Event>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Message {
    from: ClientId,
    id: MessageId,
    data: Vec<u8>,
}

impl Message {
    fn new(from: ClientId, data: Vec<u8>) -> Self {
        let id = MessageId::random();
        Self { id, from, data }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Event {
    HealthCheck,
    Connected(ClientId),
    Disconnected(ClientId),
    Message(Message),
}

struct ClientShared {
    id: ClientId,
    // Thread safe
    seen_messages: ExpiringSet<MessageId>,
    messages: RwLock<VecDeque<Message>>,
    /// Peers in whole network which are alive
    peers: RwLock<HashSet<ClientId>>,
    /// Peers to which we are connected
    connected: RwLock<HashMap<ClientId, UnboundedSender<Event>>>,
}

const MESSAGE_TTL: Duration = Duration::from_secs(60);

async fn send(
    stream: &mut TcpStream,
    message: impl ser::Serialize,
) -> Result<()> {
    let mut vec = SIGNATURE.to_vec();
    serde_cbor::to_writer(&mut vec, &message)?;

    loop {
        stream.writable().await?;
        match stream.write_all(&*vec).await {
            Ok(_) => return Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}

async fn recieve<T: de::DeserializeOwned>(stream: &TcpStream) -> Result<T> {
    let mut buf = [0; 4096];
    let num_bytes = loop {
        stream.readable().await?;
        match stream.try_read(&mut buf) {
            Ok(n) => break n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    };

    if buf.len() < SIGNATURE.len() || !buf.starts_with(SIGNATURE) {
        return Err(Error::SignatureError);
    }
    let buf = &buf[SIGNATURE.len()..num_bytes];
    Ok(serde_cbor::from_reader(buf)?)
}

impl ClientShared {
    fn new() -> Self {
        let id = ClientId::random();
        info!("My id is {:?}", id);
        let seen_messages = ExpiringSet::new(MESSAGE_TTL);
        let peers = RwLock::new(Default::default());
        let connected = RwLock::new(Default::default());
        let messages = RwLock::new(Default::default());
        Self {
            id,
            seen_messages,
            peers,
            connected,
            messages,
        }
    }

    async fn accept(
        &self,
        stream: &mut TcpStream,
        peers: HashSet<ClientId>,
    ) -> Result<(ClientId, HashSet<ClientId>)> {
        let their = recieve(stream).await?;
        send(stream, (self.id, peers)).await?;
        Ok(their)
    }

    async fn connect(
        &self,
        stream: &mut TcpStream,
        peers: HashSet<ClientId>,
    ) -> Result<(ClientId, HashSet<ClientId>)> {
        send(stream, (self.id, peers)).await?;
        recieve(stream).await
    }

    async fn propogate_event(&self, event: Event) -> Result<()> {
        self.connected
            .read()
            .await
            .values()
            .map(|ch| Ok(ch.send(event.clone())?))
            .collect::<Result<Vec<()>>>()
            .map(|_| ())
    }

    async fn handle_event(&self, event: Event) -> Result<()> {
        use Event::*;

        match event {
            // if healthcheck ignore event
            HealthCheck => (),
            // if id is already disconnected ignore event
            Disconnected(id) if !self.peers.read().await.contains(&id) => (),
            // if id is already connected ignore event
            Connected(id) if self.peers.read().await.contains(&id) => (),
            // if message is already seen ignore event
            Message(m) if self.seen_messages.contains(&m.id).await => (),

            Disconnected(id) => {
                info!("Disconnected from {:?}", id);
                self.peers.write().await.remove(&id);
                self.propogate_event(Disconnected(id)).await?;
            }
            Connected(id) => {
                info!("Connected to {:?}", id);
                self.peers.write().await.insert(id);
                self.propogate_event(Connected(id)).await?;
            }
            Message(m) => {
                self.seen_messages.insert(m.id).await;
                info!("Recieved message {:?}", &m);
                self.messages.write().await.push_back(m.clone());
                self.propogate_event(Message(m)).await?;
            }
        }

        Ok(())
    }

    #[instrument(skip(self, stream))]
    async fn main_loop(
        &self,
        id: ClientId,
        mut stream: TcpStream,
    ) -> Result<()> {
        async fn recieve_tcp(stream: &mut TcpStream) -> Result<Event> {
            recieve(stream).await
        }

        let (send_events, mut recv) = mpsc::unbounded_channel();
        self.connected.write().await.insert(id, send_events);

        let default_count = 5u32;
        let mut count = default_count;

        while count > 0 {
            select! {
                event = recv.recv() => {
                    match event {
                        // If channel is closed that means that peer is no
                        // longer alive so we need to send everyone that node
                        // is disconnected. Also we are no longer needed.
                        None => break,
                        // If recieved some event then we send it to peer
                        Some(event) => {
                            match send(&mut stream, event).await {
                                Ok(()) => count = default_count,
                                Err(error) => {
                                    count -= 1;
                                    error!(?error, count);
                                },
                            }
                        },
                    }
                }
                event = recieve_tcp(&mut stream) => {
                    match event {
                        Err(error) => {
                            count -= 1;
                            error!(?error, count);
                            continue;
                        }
                        Ok(event) => if let Err(error) = self.handle_event(event).await {
                            error!(?error);
                        },
                    }
                }
            }
        }

        // We couldn't contact peer. So we send that it disconnected and exit
        let out = self.propogate_event(Event::Disconnected(id)).await;
        self.peers.write().await.remove(&id);
        out
    }
}

pub struct Client {
    /// Shared client state
    shared: Arc<ClientShared>,
}

const SIGNATURE: &[u8] = b"gossip-client:";

impl Client {
    pub fn new(addr: SocketAddr) -> Result<Self> {
        async fn listen(
            addr: SocketAddr,
            shared: Arc<ClientShared>,
        ) -> Result<()> {
            let socket = TcpSocket::new_v4()?;
            socket.bind(addr)?;
            let listener = socket.listen(1024)?;

            loop {
                let (mut stream, _) = listener.accept().await?;
                let shared = Arc::clone(&shared);
                let fut = async move {
                    let id = {
                        let mut write = shared.peers.write().await;
                        let (id, their) =
                            shared.accept(&mut stream, write.clone()).await?;
                        write.insert(id);
                        write.extend(their);
                        id
                    };
                    if let Err(error) =
                        shared.propogate_event(Event::Connected(id)).await
                    {
                        error!(?error);
                    }
                    shared.main_loop(id, stream).await
                };
                tokio::spawn(fut);
            }
        }

        let shared = Arc::new(ClientShared::new());

        let shared_clonned = Arc::clone(&shared);
        tokio::spawn(listen(addr, shared_clonned));

        Ok(Self { shared })
    }

    /// Connects to new node
    pub async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        let mut stream = TcpStream::connect(addr).await?;
        let id = {
            let mut peers = self.shared.peers.write().await;
            let (id, their_peers) =
                self.shared.connect(&mut stream, peers.clone()).await?;
            if let Err(error) =
                self.shared.propogate_event(Event::Connected(id)).await
            {
                error!(?error);
            }

            peers.insert(id);
            peers.extend(their_peers);
            id
        };

        let shared_cloned = Arc::clone(&self.shared);
        tokio::spawn(async move { shared_cloned.main_loop(id, stream).await });

        Ok(())
    }

    /// Checks whether new messages arrived.
    /// XXX: Probably this should be blocking api
    pub async fn poll_message(&self) -> Option<Message> {
        self.shared.messages.write().await.pop_front()
    }

    /// Broadcasts message to all peers
    pub async fn send_data(&self, data: &[u8]) {
        let m = Message::new(self.shared.id, data.to_vec());
        info!("Sending {:?}", m);
        self.shared.seen_messages.insert(m.id).await;
        let out = self.shared.propogate_event(Event::Message(m)).await;
        if let Err(error) = out {
            error!(?error);
        }
    }
}
