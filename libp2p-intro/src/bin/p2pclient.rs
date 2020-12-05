use anyhow::Error;
use libp2p::{
    core::{connection::ConnectionLimits, upgrade},
    floodsub::{self, Floodsub, FloodsubEvent},
    identity::Keypair,
    mdns::{MdnsEvent, TokioMdns},
    mplex, noise,
    ping::PingConfig,
    ping::{Ping, PingEvent},
    swarm::{NetworkBehaviourEventProcess, SwarmBuilder, SwarmEvent},
    tcp, yamux, NetworkBehaviour, PeerId, Swarm, Transport,
};
use log::{debug, error, info, trace};
use std::{collections::HashSet, convert::TryInto, fmt::Debug, time::Duration};
use structopt::StructOpt;
use tokio::{
    io::{self, AsyncBufReadExt, BufReader},
    stream::StreamExt,
};

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
    dns: TokioMdns,
    ping: Ping,
    #[behaviour(ignore)]
    peers: HashSet<PeerId>,
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
                    std::string::String::from_utf8_lossy(&m.data)
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
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                if !self.peers.contains(&peer_id) {
                    self.topics.add_node_to_partial_view(peer_id.clone());
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
}

#[tokio::main]
async fn main() -> Result<(), Error> {
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

    let behaviour = OurNetwork {
        topics: pubsub,
        dns: TokioMdns::new()?,
        peers: HashSet::new(),
        ping: Ping::new(
            PingConfig::new()
                .with_interval(Duration::from_secs(5))
                .with_max_failures(3.try_into()?)
                .with_keep_alive(true),
        ),
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
                            swarm.topics.publish(topic.clone(), line);
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
