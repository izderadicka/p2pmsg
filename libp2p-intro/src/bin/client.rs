use anyhow::Error;
use futures::{future::poll_fn, stream::StreamExt};
use libp2p::{
    core::muxing::StreamMuxerEvent, core::transport::ListenerEvent, core::upgrade,
    core::StreamMuxer, identity::Keypair, noise, tcp, yamux, Transport, mplex,
};
use std::{sync::Arc, time::Duration};
use structopt::StructOpt;

const ADDR: &str = "/ip4/127.0.0.1/tcp/8000";
const TIMEOUT_SECS: u64 = 20;

#[derive(StructOpt, Debug)]
struct Args {
    #[structopt(long, short, possible_values=&["D", "L"])]
    pub mode: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::from_args();
    println!("Started");
    let key = Keypair::generate_ed25519();
    let my_id = key.public().into_peer_id();
    println!("My id is {}", my_id.to_base58());
    let transport = tcp::TokioTcpConfig::new().nodelay(true);
    let noise_key = noise::Keypair::<noise::X25519Spec>::new().into_authentic(&key)?;
    let noise = noise::NoiseConfig::xx(noise_key).into_authenticated();
    let mux = yamux::YamuxConfig::default();

    let proto = transport
        .upgrade(upgrade::Version::V1)
        .authenticate(noise)
        .multiplex(mux)
        .timeout(Duration::from_secs(TIMEOUT_SECS));

    match args.mode.as_str() {
        "D" => {
            let (peer_id, mux) = proto.dial(ADDR.parse()?)?.await?;
            println!("Connected to peer {}", peer_id);
            let mut ob = mux.open_outbound();
            println!("Opening outbound");
            let mut substream = poll_fn(|cx| mux.poll_outbound(cx, &mut ob)).await?;
            let msg = "Hello world";
            println!("Substream created");

            let sent = poll_fn(|cx| mux.write_substream(cx, &mut substream, msg.as_bytes())).await?;
            poll_fn(|cx| mux.shutdown_substream(cx, &mut substream)).await?;
            poll_fn(|cx| mux.flush_all(cx)).await?;
            poll_fn (|cx| mux.close(cx)).await?;
            println!("Sent {} bytes", sent);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        "L" => {
            let mut listener = proto.listen_on(ADDR.parse()?)?;
            while let Some(c) = listener.next().await {
                match c {
                    Err(e) => {
                        println!("Hard Error in new connection: {}", e)
                    }
                    Ok(ListenerEvent::NewAddress(addr)) => {
                        println!("Listening on : {}", addr)
                    }

                    Ok(ListenerEvent::Error(e)) => {
                        println!("Soft Error in new connection: {}", e)
                    }

                    Ok(ListenerEvent::AddressExpired(addr)) => {
                        println!("Stop listening on: {}", addr)
                    }

                    Ok(ListenerEvent::Upgrade {
                        local_addr: _,
                        remote_addr,
                        upgrade,
                    }) => {
                        let handle_conn = async move {
                            let conn = upgrade.await;
                            
                            match conn {
                                Err(e) => println!("Upgrade failed! {}", e),
                                Ok((peer_id, conn)) => {
                                    let conn = Arc::new(conn);
                                    println!("Connected to peer {} at {}", peer_id, remote_addr);
                                    loop {
                                        match poll_fn(|cx| conn.poll_event(cx)).await {
                                            Err(e) => {
                                                println!("Error getting substream {}", e);
                                                break;
                                            }

                                            Ok(StreamMuxerEvent::InboundSubstream(mut s)) => {
                                                println!("Got substream");
                                                let conn2 = conn.clone();
                                                let handle_substream = async move {
                                                    let mut buf = [0; 1024];
                                                    loop {
                                                        match poll_fn(|cx| {
                                                            conn2.read_substream(
                                                                cx, &mut s, &mut buf,
                                                            )
                                                        })
                                                        .await
                                                        {
                                                            Ok(read) => {
                                                                if read == 0 {
                                                                    break;
                                                                }
                                                                let msg = std::str::from_utf8(
                                                                    &buf[..read],
                                                                )
                                                                .unwrap();
                                                                println!("Got msg {}", msg);
                                                            }
                                                            Err(e) => {
                                                                println!(
                                                                    "Error reading substream {}",
                                                                    e
                                                                );
                                                                break;
                                                            }
                                                        }
                                                    }
                                                };

                                                tokio::spawn(handle_substream);
                                            }

                                            Ok(StreamMuxerEvent::AddressChange(addr)) => {
                                                println!("Peer address change to {}", addr);
                                            }
                                        }
                                    }
                                }
                            }
                        };
                        tokio::spawn(handle_conn);
                    }
                }
            }
        }
        _ => unreachable!("BUG: invalid arg"),
    }

    println!("Finished");
    Ok(())
}
