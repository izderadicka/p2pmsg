#![allow(clippy::new_without_default)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;

pub mod protocol;
pub mod error;
pub mod client;

pub use crate::client::run_client;

