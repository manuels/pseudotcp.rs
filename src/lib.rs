#![feature(arc_weak)]
#![allow(dead_code)]

#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate log;
extern crate libc;
extern crate env_logger;
extern crate time;
extern crate condition_variable;

mod bindings;
mod api;
mod pseudo_tcp_socket;

#[cfg(test)]
mod tests;

pub use pseudo_tcp_socket::{PseudoTcpSocket, PseudoTcpStream};
