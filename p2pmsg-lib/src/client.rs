use futures::{future, stream::StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock, oneshot};
use tokio_util::codec::Decoder;

use crate::error::Error;
use crate::protocol::codec::MsgCodec;
use crate::protocol::message::Message;
use futures::{join, prelude::*};
use std::time::Instant;
use future::Either;

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    id: String,
    addr: SocketAddr,
    name: String,
    uses_nat: bool,
}

#[allow(dead_code)]
pub struct ActivePeer {
    //last_ping_ts: Instant,
    //last_ping_id: [u8; 32],
    adr: SocketAddr,
    //info: PeerInfo,
    terminator: ActivePeerTerminator,
    writer: PeerWriter
}

impl ActivePeer {

    async fn send(&mut self, m: Message) -> Result<(), Error>{
        self.writer.send(m).await
    }

    fn close(self) -> Result<(),Error> {
        self.terminator.send(self.writer).map_err(|_| "cannot send via oneshot channel".into())
    }
}

type PeerWriter =
    futures::stream::SplitSink<tokio_util::codec::Framed<tokio::net::TcpStream, MsgCodec>, Message>;
type ActivePeerTerminator = oneshot::Sender<PeerWriter>;



#[derive(Clone)]
pub struct OpenConnections {
    sinks: Arc<RwLock<HashMap<SocketAddr, ActivePeer>>>,
}

impl OpenConnections {
    pub fn new() -> Self {
        OpenConnections {
            sinks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_new(&self, peer: SocketAddr, writer: PeerWriter, terminator: ActivePeerTerminator) {
        let mut sinks = self.sinks.write().await;
        sinks.insert(peer.clone(), ActivePeer{adr: peer, writer, terminator});
    }

    pub async fn remove(&self, peer: &SocketAddr) -> Option<ActivePeer> {
        let mut sinks = self.sinks.write().await;
        sinks.remove(peer)
    }

    pub async fn send(&self, to: SocketAddr, msg: Message) -> Result<(), Error> {
        match OPEN_CONNECTION.sinks.write().await.get_mut(&to) {
            Some(s) => s.send(msg).await,
            None => Err(format!("Connection to {} is not available ", &to).into()),
        }
    }
}

async fn handle_connection(
    socket: TcpStream,
    mut tx: tokio::sync::mpsc::Sender<(Message, std::net::SocketAddr)>,
) {
    let peer = socket.peer_addr().unwrap();
    info!("Connected by client {:?}", peer);
    let (mut writer, mut reader) = MsgCodec::new().framed(socket).split();
    let my_hello = Message::Hello {
        msg: "Hello from me".into(),
    };
    let (terminator, mut terminator_receiver) = oneshot::channel();

    let receiving_loop_future = async move {
        match writer.send(my_hello).await {
            Ok(()) => {
                match reader.next().await {
                    Some(Ok(Message::Hello { msg })) => {
                        debug!("Client {} connected with hello message {}", peer, msg);
                        OPEN_CONNECTION.add_new(peer, writer, terminator).await;
                    }
                    _ => error!("invalid handshake"),
                };

                

                 loop {

                    match future::select(reader.next(), &mut terminator_receiver).await {
                        Either::Left((Some(m), _)) => match m {
                            Ok(m) => {
                                if let Err(_) = tx.send((m, peer.clone())).await {
                                    error!("internal error in incoming channel");
                                }
                            }
                            
                            Err(e) => error!("error in incoming stream {}", e)
                        }

                        Either::Left((None, _)) => break,
                        Either::Right((Ok(mut writer), _)) => {
                            if let Err(e) = writer.send(Message::Terminate).await {
                                error!("Cannot send final message {}", e);
                            };

                            if let Ok(s) =  writer.reunite(reader) {
                                s.get_ref().shutdown(std::net::Shutdown::Both).unwrap_or_else(|e| error!("cannot shutdown socket {}", e));
                            } else {
                                error!("error in reunite!")
                            }
                            break
                        }
                        Either::Right((Err(e), _)) => {
                            error!("terminator error {}", e);
                            break
                        }

                    }
                }
                    

                let _p = OPEN_CONNECTION.remove(&peer).await;
                
                debug!("Connection done for {}", peer);
            }
            Err(e) => error!("error sending hello message {}", e),
        }
    };

    tokio::spawn(receiving_loop_future);
}

lazy_static! {
    static ref OPEN_CONNECTION: OpenConnections = OpenConnections::new();
}

pub async fn run_client(port: u16, peers: Option<Vec<SocketAddr>>) -> Result<(), Error> {
    info!("Started client on port {}", port);
    let (tx, mut rx) = mpsc::channel(1024);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut server = TcpListener::bind(&addr).await?;

    let tx2 = tx.clone();
    let server_loop = server
        .incoming()
        .filter_map(|s| {
            future::ready(
                s.map_err(|e| error!("error accepting incoming stream: {}", e))
                    .ok(),
            )
        })
        .for_each(move |socket| handle_connection(socket, tx.clone()));

    let receiving_loop = async {
        while let Some((msg, peer)) = rx.next().await {
            debug!("Received message {:#?} from {:?}", msg, peer);
            use self::Message::*;
            match msg {
                Hello { .. } => {
                    error!("should not receive hello here");
                }
                Ping => OPEN_CONNECTION
                    .send(peer, Pong)
                    .await
                    .unwrap_or_else(|e| error!("Pong send error {}", e)),
                Pong => {
                    info!("Got Pong");
                }
                Terminate => {
                    info!("Got Terminate");
                    if let Some(ap) = OPEN_CONNECTION.remove(&peer).await {
                        ap.close()
                            .unwrap_or_else(|e| error!("cannot close writer: {}", e));
                    };
                }
            };
        }
    };

    let connect_known = async {
        if let Some(peers) = peers {
            for addr in peers {
                let tx3 = tx2.clone();
                tokio::spawn(async move {
                    match TcpStream::connect(&addr).await {
                        Ok(socket) => handle_connection(socket, tx3).await,
                        Err(e) => error!("Connect error {}", e),
                    }
                });
            }
        }
    };

    join!(server_loop, receiving_loop, connect_known);
    Ok(())
}
