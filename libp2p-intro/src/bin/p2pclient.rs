#![feature(min_specialization)]

use anyhow::Error;
use libp2p::{
    core::{connection::ConnectionLimits, upgrade},
    floodsub::{self, Floodsub, FloodsubEvent},
    identity::Keypair,
    mdns::{MdnsEvent, TokioMdns},
    noise,
    ping::PingConfig,
    ping::{Ping, PingEvent},
    swarm::{NetworkBehaviourEventProcess, SwarmBuilder, SwarmEvent},
    tcp, yamux, NetworkBehaviour, PeerId, Swarm, Transport,
};

use libp2p::kad::{
    record::store::MemoryStore, record::Key, AddProviderOk, Kademlia, KademliaEvent, PeerRecord,
    PutRecordOk, QueryResult, Quorum, Record,
};

use log::{debug, error, info, trace};
use std::{borrow::Cow, collections::HashSet, convert::TryInto, fmt::Debug, time::Duration};
use structopt::StructOpt;
use tokio::{
    io::{self, AsyncBufReadExt, BufReader},
    stream::StreamExt,
};

type Result<T> = std::result::Result<T, anyhow::Error>;

const ADDR: &str = "/ip4/127.0.0.1/tcp/0";
const TIMEOUT_SECS: u64 = 20;

#[derive(StructOpt, Debug)]
struct Args {
    #[structopt(name = "PEER_ADDR")]
    pub peers: Vec<String>,

    #[structopt(long, short)]
    pub no_input: bool,
}

#[derive(NetworkBehaviour)]
struct OurNetwork {
    topics: Floodsub,
    #[behaviour(ignore)]
    topic: floodsub::Topic,
    dns: TokioMdns,
    kad: Kademlia<MemoryStore>,
    ping: Ping,
    #[behaviour(ignore)]
    peers: HashSet<PeerId>,
    #[behaviour(ignore)]
    my_id: PeerId,
    #[behaviour(ignore)]
    kad_boostrap_started: bool
}


trait Printable {
    fn printable(&self) ->  Cow<str>; 
}

trait PrintableList<'a> {
    fn printable_list(&'a self) -> String;
}

impl <T> Printable for T where T:AsRef<[u8]> {
    default fn printable(&self) -> Cow<str> {
        std::string::String::from_utf8_lossy(self.as_ref())
    }
}

impl <'a, T> PrintableList<'a> for T 
where &'a T: IntoIterator,
T: 'a,
<&'a T as IntoIterator>::Item: std::string::ToString {

    fn printable_list(&'a self) -> String {
        self.into_iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ")
    }

}

impl NetworkBehaviourEventProcess<KademliaEvent> for OurNetwork {
    fn inject_event(&mut self, evt: KademliaEvent) {
        match evt {
            KademliaEvent::QueryResult { result, .. } => match result {
                QueryResult::GetProviders(Ok(ok)) => {
                    debug!("Got providers {:?}", ok);
                    println!(
                        "Key {} is provided by ({})",
                        ok.key.printable(),
                        ok.providers.printable_list()
                    );
                }
                QueryResult::GetProviders(Err(err)) => {
                    error!("Failed to get providers: {:?}", err);
                }
                QueryResult::GetRecord(Ok(ok)) => {
                    debug!("Got record: {:?}", ok);
                    for PeerRecord {
                        record: Record { key, value, .. },
                        ..
                    } in ok.records
                    {
                        println!(
                            "Record {:?} = {:?}",
                            key.printable(),
                            value.printable(),
                        );
                    }
                }
                QueryResult::GetRecord(Err(err)) => {
                    error!("Failed to get record: {:?}", err);
                }
                QueryResult::PutRecord(Ok(PutRecordOk { key })) => {
                    debug!(
                        "Successfully put record {:?}",
                        key.printable()
                    );
                }
                QueryResult::PutRecord(Err(err)) => {
                    error!("Failed to put record: {:?}", err);
                }
                QueryResult::StartProviding(Ok(AddProviderOk { key })) => {
                    debug!(
                        "Successfully put provider record {:?}",
                        key.printable()
                    );
                }
                QueryResult::StartProviding(Err(err)) => {
                    error!("Failed to put provider record: {:?}", err);
                }

                QueryResult::GetClosestPeers(Ok(res)) => {
                    debug!("Got closest peers {:?}", res);
                    println!("Closest peers for {} are ({})",
                    res.key.printable(),
                    res.peers.printable_list()
                    )
                }
                QueryResult::GetClosestPeers(Err(e)) => {
                    error!("Error getting closest peers: {:?}", e);
                }
                QueryResult::Bootstrap(Ok(boot))=> {
                    debug!("Boostrap done: {:?}", boot)
                }
                QueryResult::Bootstrap(Err(e)) => {
                    error!("Boostrap error: {:?}", e);
                }
                QueryResult::RepublishProvider(res) => {
                    match res {
                        Ok(r) => debug!("Republished provider: {:?}", r),
                        Err(e) => error!("Error republishing provider: {:?}", e)
                    }
                }
                QueryResult::RepublishRecord(res) => {
                    match res {
                        Ok(r) => debug!("Republished record: {:?}", r),
                        Err(e) => error!("Error republishing record: {:?}", e)
                    }
                }
            },
            KademliaEvent::RoutablePeer{peer, address} => {
                debug!("Peer discovered {} at {}, but is not added to table", peer, address)
            }
            KademliaEvent::PendingRoutablePeer{peer, address} => {
                debug!("Peer discovered {} at {}, and might be added to table later", peer, address)
            }
            KademliaEvent::RoutingUpdated{peer, addresses, old_peer} => {
                debug!("Routing table updated by {} at {}, replacing {:?}", peer, addresses.into_vec().printable_list(), old_peer);
                if !self.kad_boostrap_started {
                    self.kad.bootstrap().unwrap();
                    self.kad_boostrap_started = true;
                }
            }
            KademliaEvent::UnroutablePeer{peer} => {
                debug!("Unroutable peer {}", peer)
            }
        }
    }
}

impl NetworkBehaviourEventProcess<PingEvent> for OurNetwork {
    fn inject_event(&mut self, evt: PingEvent) {
        trace!("Got ping event {:?}", evt);
    }
}

impl NetworkBehaviourEventProcess<MdnsEvent> for OurNetwork {
    fn inject_event(&mut self, evt: MdnsEvent) {
        match evt {
            MdnsEvent::Discovered(peers) => {
                for (peer, addr) in peers {
                    if !self.peers.contains(&peer) {
                        debug!("Discovered peer {} on {}", peer, addr);
                        self.topics.add_node_to_partial_view(peer.clone());
                        self.kad.add_address(&peer, addr);
                        self.peers.insert(peer);
                    } else {
                        trace!("mDNS: found known peer{} on {}", peer, addr);
                    }
                }
            }
            MdnsEvent::Expired(peers) => {
                for (peer, addr) in peers {
                    debug!("Peer {} on {} expired and removed", peer, addr);
                }
            }
        }
    }
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for OurNetwork {
    fn inject_event(&mut self, evt: FloodsubEvent) {
        match evt {
            FloodsubEvent::Message(m) => {
                debug!("Received message {:?}", m);
                println!(
                    "({})->{}",
                    m.source,
                    m.data.printable()
                )
            }
            FloodsubEvent::Subscribed { peer_id, topic } => {
                debug!("Peer {} subscribed to {:?}", peer_id, topic)
            }
            FloodsubEvent::Unsubscribed { peer_id, topic } => {
                debug!("Peer {} unsubscribed from {:?}", peer_id, topic)
            }
        }
    }
}

impl OurNetwork {
    fn handle_event<T: Debug, U: Debug>(&mut self, event: SwarmEvent<T, U>) {
        debug!("Swarm event {:?}", event);
        match event {
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                if !self.peers.contains(&peer_id) {
                    self.topics.add_node_to_partial_view(peer_id.clone());

                    self.kad
                        .add_address(&peer_id, endpoint.get_remote_address().clone());

                    self.peers.insert(peer_id);
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                ..
            } => {
                if num_established == 0 && self.peers.contains(&peer_id) {
                    self.topics.remove_node_from_partial_view(&peer_id);
                    self.peers.remove(&peer_id);
                }
            }

            _ => {}
        }
    }

    fn handle_input(&mut self, line: String) -> Result<()> {
        let mut items = line.split(' ').filter(|s| !s.is_empty());

        let cmd = next_item(&mut items, "command")?.to_ascii_uppercase();
        match cmd.as_str() {
            "PUT" => {
                let key = next_item(&mut items, "key")?;
                let value = rest_of(items)?;
                let record = Record {
                    key: Key::new(&key),
                    value: value.into(),
                    publisher: None,
                    expires: None,
                };
                self.kad
                    .put_record(record, Quorum::One)
                    .map_err(|e| Error::msg(format!("Store error {:?}", e)))?;
            }
            "GET" => {
                let key = Key::new(&next_item(&mut items, "key")?);
                self.kad.get_record(&key, Quorum::One);
            }
            "FLOOD" => {
                self.topics.publish(self.topic.clone(), rest_of(items)?);
            }

            "PROVIDE" => {
                let key = Key::new(&next_item(&mut items, "key")?);
                self.kad
                    .start_providing(key)
                    .map_err(|e| Error::msg(format!("Store error {:?}", e)))?;
            }
            "STOP_PROVIDE" => {
                let key = Key::new(&next_item(&mut items, "key")?);
                self.kad.stop_providing(&key);
            }
            "GET_PROVIDERS" => {
                let key = Key::new(&next_item(&mut items, "key")?);
                self.kad.get_providers(key);
            }
            "GET_PEERS" => {
                let key = Key::new(&next_item(&mut items, "key")?);
                self.kad.get_closest_peers(key);
            }
            "MY_ID" => {
                println!("My ID is {}", self.my_id)
            }
            _ => error!("Invalid command {}", cmd),
        }

        Ok(())
    }
}

fn next_item<'a, I: Iterator<Item = &'a str>>(items: &mut I, what: &str) -> Result<&'a str> {
    if let Some(item) = items.next() {
        Ok(item)
    } else {
        Err(Error::msg(format!("Expected {} in input line", what)))
    }
}

fn rest_of<'a, I: Iterator<Item = &'a str>>(items: I) -> Result<String> {
    let txt: Vec<_> = items.collect();
    let txt = txt.join(" ");
    if txt.is_empty() {
        Err(Error::msg(format!("Empty value")))
    } else {
        Ok(txt)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::try_init()?;
    let args = Args::from_args();
    info!("Started");
    let key = Keypair::generate_ed25519();
    let my_id = key.public().into_peer_id();
    println!("My id is {}", my_id.to_base58());
    info!("My id is {}", my_id.to_base58());
    let transport = tcp::TokioTcpConfig::new().nodelay(true);
    let noise_key = noise::Keypair::<noise::X25519Spec>::new().into_authentic(&key)?;
    let noise = noise::NoiseConfig::xx(noise_key).into_authenticated();
    let mux = yamux::YamuxConfig::default();

    let proto = transport
        .upgrade(upgrade::Version::V1)
        .authenticate(noise)
        .multiplex(mux)
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .boxed();

    let topic = floodsub::Topic::new("test_chat");

    let mut pubsub = Floodsub::new(my_id.clone());
    pubsub.subscribe(topic.clone());

    let kad = Kademlia::new(my_id.clone(), MemoryStore::new(my_id.clone()));

    let behaviour = OurNetwork {
        topics: pubsub,
        topic,
        dns: TokioMdns::new()?,
        peers: HashSet::new(),
        ping: Ping::new(
            PingConfig::new()
                .with_interval(Duration::from_secs(5))
                .with_max_failures(3.try_into()?)
                .with_keep_alive(true),
        ),
        kad,
        kad_boostrap_started: false,
        my_id: my_id.clone()
    };

    let mut swarm = SwarmBuilder::new(proto, behaviour, my_id)
        .executor(Box::new(|f| {
            tokio::spawn(f);
        }))
        .connection_limits(ConnectionLimits::default())
        .build();

    for peer_addr in args.peers {
        let addr = peer_addr.parse()?;
        Swarm::dial_addr(&mut swarm, addr)?;
    }

    let _listener_id = Swarm::listen_on(&mut swarm, ADDR.parse().unwrap())?;

    if args.no_input {
        loop {
            let evt = swarm.next_event().await;
            swarm.handle_event(evt)
        }
    } else {
        let mut input = BufReader::new(io::stdin()).lines();

        loop {
            tokio::select! {
                line = input.next() => {
                    match line {
                        Some(Ok(line)) => {

                            swarm.handle_input(line).unwrap_or_else(|e| error!("Input error: {}", e));
                        },
                        None => {
                            debug!("End of stdin");
                            break
                        }
                        Some(Err(e)) => {
                            error!("error reading stdin: {}", e);
                        }
                    }
                }
                event = swarm.next_event() => {
                    swarm.handle_event(event)
                }
            }
        }
    }

    info!("Finished");
    Ok(())
}
