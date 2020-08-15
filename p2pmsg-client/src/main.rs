#[macro_use]
extern crate clap;
#[macro_use]
extern crate log;

use p2pmsg_lib::error::Error;
use p2pmsg_lib::run_client;

mod cmd {
    use clap::{App, Arg};
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::fmt::Debug;

    #[derive(Debug)]
    pub struct Config {
        pub port: u16,
        pub peers: Option<Vec<SocketAddr>>,
    }

    fn validator<T>(s: String) -> Result<(), String> 
     where T:FromStr, <T as FromStr>::Err :Debug
    {
        s.parse()
            .map(|_i: T| ())
            .map_err(|e| format!("{:?}",e))
    }

    fn args_parser<'a, 'b>() -> App<'a, 'b> {
        app_from_crate!()
            .arg(
                Arg::with_name("port")
                    .short("p")
                    .long("port")
                    .takes_value(true)
                    .validator(validator::<u16>)
                    .default_value("12345"),
            )
            .arg(
                Arg::with_name("peer")
                    .long("peer")
                    .takes_value(true)
                    .multiple(true)
                    .validator(validator::<SocketAddr>),
            )
    }

    pub fn parse_args<'a>() -> Config {
        let args = args_parser().get_matches();
        let port: u16 = args.value_of("port").unwrap().parse().unwrap();
        let peers = args
            .values_of("peer")
            .map(|peers| peers.map(|p| p.parse().unwrap()).collect());

        Config { port, peers }
    }
}


#[tokio::main]
async fn main() -> Result<(), Error> {
    let cfg = cmd::parse_args();
    env_logger::init();
    info!("Program arguments {:?}", &cfg);
    run_client(cfg.port, cfg.peers).await

    //Ok(())
}
