#[allow(unused_imports)]

use std::sync::{Once, ONCE_INIT};
use std::sync::mpsc::channel;
use std::io;
use std::io::{Read, Write};
use std::error::Error;
use std::thread;

use env_logger;

use std::time::{Instant, Duration};
use pseudo_tcp_socket::{PseudoTcpStream, PseudoTcpSocket};
use bindings as ffi;

static START: Once = ONCE_INIT;

#[test]
fn test_stream() {
	START.call_once(|| {
		env_logger::init().unwrap();
	});

	fn send(tcp: &mut PseudoTcpStream, buf: &[u8]) -> usize {
		let mut pos = 0;

		while pos < buf.len() {
			match tcp.write(&buf[pos..]) {
				Ok(len) => {
					pos += len;
				},
				Err(error) => {
					if error.kind() == io::ErrorKind::WouldBlock {
						continue
					}
					panic!("{:?} {:?}", error.kind(), error.description());
				}
			}
		}

		buf.len()
	}

	let (tx_a, rx_b) = channel();
	let (tx_b, rx_a) = channel();

	PseudoTcpStream::set_debug_level(ffi::PSEUDO_TCP_DEBUG_VERBOSE);

	let mut bob   = PseudoTcpSocket::listen((tx_b, rx_b), String::from("bob"));
	let mut alice = PseudoTcpSocket::connect((tx_a, rx_a), String::from("alice")).unwrap();

	alice.notify_mtu(1496);
	bob.notify_mtu(1496);

	let n_iter = 10;

	thread::spawn(move || {
		bob.wait_for_connection(4000);

		let mut write_count = 0;
		let mut is_connected = true;

		while is_connected {
			let mut buf = vec![0; 10*1024];

			match bob.read(&mut buf[..]) {
				Ok(len)  => {
					buf.truncate(len);

					assert!(buf.len() > 0);
					assert!(buf.iter().all(|x| *x == 0x0));

					send(&mut bob, &buf[..]);

					write_count += buf.len();
				}
				Err(err) => {
					debug!("bob.read = {:?}", err);
					thread::sleep(Duration::from_millis(100));

					match err.kind() {
						io::ErrorKind::WouldBlock => continue,
						io::ErrorKind::NotConnected => is_connected = false,
						_ => panic!(err),
					}
				}
			}
		}
		debug!("bob sent {} bytes", write_count);

		bob.close(false);

		thread::sleep(Duration::from_millis(500));
	});
	
	alice.wait_for_connection(4000);

	let mut write_count = 0;
	let mut read_count = 0;

	let mut i = 0;
	let mut is_connected = true;
	while is_connected {
		let mut buf = vec![0; 10*1024];

		if i < n_iter {
			write_count += send(&mut alice, &buf[..]);
		} else if i == n_iter {
			// give bob time to reply the data sent last
			thread::sleep(Duration::from_millis(1000));

			debug!("alice.close()");
			alice.close(false);
		}
		i += 1;

		match alice.read(&mut buf[..]) {
			Ok(len)  => {
				buf.truncate(len);

				assert!(buf.len() > 0);
				assert!(buf.iter().all(|x| *x == 0x0));

				read_count += buf.len();
			},
			Err(err) => {
				debug!("alice.read = {:?}", err);
				thread::sleep(Duration::from_millis(100));

				match err.kind() {
					io::ErrorKind::WouldBlock => continue,
					io::ErrorKind::NotConnected => is_connected = false,
					_ => panic!(err),
				}
			}
		}
	}

	info!("alice done and sent {} bytes.", write_count);
	info!("alice done and read {} bytes.", read_count);

	assert_eq!(read_count, write_count);
}

#[test]
fn test_channel() {
	START.call_once(|| {
		env_logger::init().unwrap();
	});

	let (tx_a, rx_b) = channel();
	let (tx_b, rx_a) = channel();

	PseudoTcpStream::set_debug_level(ffi::PSEUDO_TCP_DEBUG_VERBOSE);

	let bob   = PseudoTcpSocket::listen((tx_b, rx_b), String::from("bob"));
	let alice = PseudoTcpSocket::connect((tx_a, rx_a), String::from("alice")).unwrap();
	bob.wait_for_connection(4000);

	alice.notify_mtu(1496);
	bob.notify_mtu(1496);

	let n_iter = 1;

	thread::spawn(move || {
		let (tx, rx) = bob.to_channel();

		for buf in rx {
			tx.send(buf).unwrap();
		}

		thread::sleep(Duration::from_millis(1000));
		debug!("bob is done");
	});

	let (tx, rx) = alice.to_channel();

	let mut write_count = 0;
	let mut read_count = 0;

	for _ in 0..n_iter {
		let buf = vec![0; 10*1024];
		write_count += buf.len();
		tx.send(buf).unwrap();

		if let Ok(buf) = rx.try_recv() {
			read_count += buf.len();
		}
	}
	thread::sleep(Duration::from_millis(1000));

	for buf in rx {
		read_count += buf.len();

		if read_count == write_count {
			break
		}
	}

	thread::sleep(Duration::from_millis(1000));
	info!("alice done and sent {} bytes.", write_count);
	info!("alice done and read {} bytes.", read_count);

	assert_eq!(read_count, write_count);
}

#[test]
fn test_benchmark() {
	START.call_once(|| {
		env_logger::init().unwrap();
	});

	fn send(tcp: &mut PseudoTcpStream, buf: &[u8]) -> usize {
		let mut pos = 0;

		while pos < buf.len() {
			match tcp.write(&buf[pos..]) {
				Ok(len) => {
					pos += len;
				},
				Err(error) => {
					if error.kind() == io::ErrorKind::WouldBlock {
						continue
					}
					panic!("{:?} {:?}", error.kind(), error.description());
				}
			}
		}

		buf.len()
	}

	let (tx_a, rx_b) = channel();
	let (tx_b, rx_a) = channel();

	PseudoTcpStream::set_debug_level(ffi::PSEUDO_TCP_DEBUG_VERBOSE);

	let mut bob   = PseudoTcpSocket::listen((tx_b, rx_b), String::from("bob"));
	let mut alice = PseudoTcpSocket::connect((tx_a, rx_a), String::from("alice")).unwrap();

	alice.notify_mtu(1496);
	bob.notify_mtu(1496);

	thread::spawn(move || {
		bob.wait_for_connection(4000);

		let mut count = 0;
		let mut total_count = 0;
		let mut is_connected = true;

		let start = Instant::now();
		let mut last_print = start;

		while is_connected && start.elapsed() < Duration::from_secs(60) {
			let mut buf = vec![0; 10*1024];

			let last_elapsed = last_print.elapsed().as_secs();
			if last_elapsed > 0 {
				println!("bob received {} bytes in {} sec", count, last_elapsed);
				total_count += count;
				count = 0;
				last_print = Instant::now();
			}

			match bob.read(&mut buf[..]) {
				Ok(len)  => {
					buf.truncate(len);

					assert!(buf.len() > 0);
					assert!(buf.iter().all(|x| *x == 0x0));

					count += buf.len();
				}
				Err(err) => {
					debug!("bob.read = {:?}", err);
					thread::sleep(Duration::from_millis(100));

					match err.kind() {
						io::ErrorKind::WouldBlock => continue,
						io::ErrorKind::NotConnected => is_connected = false,
						_ => panic!(err),
					}
				}
			}
		}
		let elapsed_secs = start.elapsed().as_secs();
		println!("bob received {} bytes in {} sec => {} bytes/sec", total_count, elapsed_secs,
			(total_count as u64)/elapsed_secs);

		bob.close(false);

		thread::sleep(Duration::from_millis(500));
	});
	
	alice.wait_for_connection(4000);

	let mut write_count = 0;
	let mut read_count = 0;

	let mut i = 0;
	let mut is_connected = true;
	while is_connected {
		let mut buf = vec![0; 10*1024];

		write_count += send(&mut alice, &buf[..]);
	}

	info!("alice done and sent {} bytes.", write_count);
}
