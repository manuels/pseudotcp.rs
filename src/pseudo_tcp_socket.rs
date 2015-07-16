use std::io;
use std::io::{Read, Write};
use std::thread;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Sender, Receiver, channel};

use env_logger;

use api;
use bindings as ffi;
use condition_variable::{ConditionVariable, Notify};

const CONVERSATION_ID:i32 = 0xFEED;
const FIN:[u8;2] = [0xDE, 0xAD];

pub struct PseudoTcpSocket {
	tcp:           Arc<Mutex<api::PseudoTcpSocket>>,
	connected:     Arc<ConditionVariable<bool>>,
	readable:      Arc<ConditionVariable<bool>>,
	writable:      Arc<ConditionVariable<bool>>,
	adjust_clock:  Arc<ConditionVariable<()>>,
}

impl PseudoTcpSocket {
	pub fn new(ch: (Sender<Vec<u8>>, Receiver<Vec<u8>>)) -> PseudoTcpSocket {
		let (tx, rx) = ch;

		let connected = Arc::new(ConditionVariable::new(false));
		let readable  = Arc::new(ConditionVariable::new(false));
		let writable  = Arc::new(ConditionVariable::new(false));

		let callbacks = {
			let tx         = tx.clone();
			let readable   = readable.clone();
			let writable   = writable.clone();
			let connected1 = connected.clone();
			let connected2 = connected.clone();

			let write_packet = move |buf: &[u8]| {
				debug!("write_packet len={} first bytes={:x} {:x} {:x} {:x}", buf.len(), buf[0], buf[1], buf[2], buf[3]);

				match tx.send(buf.to_vec()) {
					Ok(()) => ffi::WR_SUCCESS,
					Err(err) => {
						error!("{:?}", err);
						ffi::WR_FAIL
					},
				}
			};

			api::Callbacks {
				opened:       Some(Box::new(move ||  connected1.set(true, Notify::All))),
				aborted:      Some(Box::new(move |_| {connected2.set(false, Notify::All)})),
				readable:     Some(Box::new(move ||  readable.set(true, Notify::All))),
				writable:     Some(Box::new(move ||  writable.set(true, Notify::All))),
				write_packet: Some(Box::new(write_packet)),
			}
		};

		let tcp = api::PseudoTcpSocket::new(CONVERSATION_ID, callbacks);
		let tcp = Arc::new(Mutex::new(tcp));
		let adjust_clock = Arc::new(ConditionVariable::new(()));

		let socket = PseudoTcpSocket {
			tcp:           tcp,
			readable:      readable,
			writable:      writable,
			connected:     connected,
			adjust_clock: adjust_clock,
		};

		socket.spawn_udp_receiver(rx);
		socket.spawn_clock(tx);

		socket
	}

	#[must_use]
	pub fn connect(&self) -> bool {
		let res = {
			let lock = self.tcp.lock().unwrap();
			lock.connect()
		};
		self.adjust_clock();
		
		res
	}

	pub fn close(&mut self, force: bool) {
		{
			let lock = self.tcp.lock().unwrap();
			lock.close(force);
		}

		self.adjust_clock();

		// The PseudoTcpCallbacks:PseudoTcpClosed callback will not be called
		// once the socket gets closed. It is only used for aborted connection.
		// Instead, the socket gets closed when the
		// pseudo_tcp_socket_get_next_clock() function returns FALSE.
	}

	pub fn notify_mtu(&self, mtu: u16) {
		let lock = self.tcp.lock().unwrap();
		lock.notify_mtu(mtu);
	}

	/// use environment variables G_MESSAGES_DEBUG=all NICE_DEBUG=all
	pub fn set_debug_level(level: ffi::PseudoTcpDebugLevel) {
		api::PseudoTcpSocket::set_debug_level(level)
	}

	fn adjust_clock(&self) {
		(*self.adjust_clock).set((), Notify::All)
	}

	pub fn spawn_clock(&self, tx: Sender<Vec<u8>>) -> thread::JoinHandle<()> {
		let adjust_clock = self.adjust_clock.clone();
		let connected     = self.connected.clone();
		let tcp = self.tcp.clone();

		thread::spawn(move || {
			let mut timer = Some(0);
			
			while let Some(timeout_ms) = timer {
				(*adjust_clock).wait_ms(timeout_ms as u32).unwrap();

				timer = {
					let lock = tcp.lock().unwrap();

					lock.notify_clock();
					lock.get_next_clock_ms()
				};
			}
			/* socket was closed. (see pseudo_tcp_socket_close() docs) */

			// apparently libnice v0.0.11 does not take care of telling the
			// other peer that the connection was closed.
			// This is fixed in the latest libnice version!
			// We do this by sending FIN (0xDEADFEED) over the wire. Usual
			// libnice packets begin with the conversation id (0xFEEDF00D).
			let _ = tx.send(FIN.to_vec());

			debug!("tcp socket: finished closing socket.");
			connected.set(false, Notify::All);
		})
	}

	fn spawn_udp_receiver(&self, rx: Receiver<Vec<u8>>) -> thread::JoinHandle<()> {
		let tcp = self.tcp.clone();
		let adjust_clock = self.adjust_clock.clone();

		thread::spawn(move || {
			for buf in rx {
				let lock = tcp.lock().unwrap();

				// Usual libnice packets begin with the conversation id which is
				// 0xFEEDF00D in our case. (see PseudoTcpSocket::spawn_clock())
				if buf == FIN {

					info!("Got FIN!");
					return lock.close(false);
				}

				let res = lock.notify_packet(&buf[..]);
				if !res {
					warn!("notify_packet failed!");
				}

				// the next line is the same as self.adjust_clock():
				(*adjust_clock).set((), Notify::All)
			}

			let lock = tcp.lock().unwrap();
			lock.close(false);
		})
	}
}

impl io::Read for PseudoTcpSocket {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		// do NOT wait for self.readable here: if the connection is closed,
		// we will never satisfy it!
		// self.readable.wait_for(true).unwrap();

		let res = {
			let lock = self.tcp.lock().unwrap();
			let res = lock.recv(buf);

			if let Err(ref err) = res {
				if err.kind() == io::ErrorKind::WouldBlock {
					// We must keep the lock to ensure that readable callback
					// is not called before we set self.readable to false!
					self.readable.set(false, Notify::All);
				}
			}

			res
		};
		self.adjust_clock();

		let disconnected = match res {
			Ok(len) if len == 0 => true,
			Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
				!self.connected.get().unwrap()
			},
			_ => false,
		};

		if disconnected {
			self.close(false);

			let error = io::Error::new(io::ErrorKind::NotConnected, "");
			return Err(error);
		}

		res
	}
}

impl io::Write for PseudoTcpSocket {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		// do NOT wait for self.connected here: if the connection is closed,
		// we will never satisfy it!
		// self.connected.wait_for(true).unwrap();

		let res = {
			let lock = self.tcp.lock().unwrap();
			let res = lock.send(buf);

			match res {
				Ok(len) if len < buf.len() => {
					self.writable.set(false, Notify::All);
				},
				Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
					self.writable.set(false, Notify::All);
				}
				_ => (),
			}

			res
		};
		self.adjust_clock();

		res
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(()) //TODO (is that even possible?)
	}
}

#[test]
fn test() {
	env_logger::init().unwrap();

	let (tx_a, rx_b) = channel();
	let (tx_b, rx_a) = channel();

	PseudoTcpSocket::set_debug_level(ffi::PSEUDO_TCP_DEBUG_VERBOSE);

	let mut alice = PseudoTcpSocket::new((tx_a, rx_a));
	let mut bob   = PseudoTcpSocket::new((tx_b, rx_b));

	alice.notify_mtu(1496);
	bob.notify_mtu(1496);

	assert!(alice.connect());
	// TODO:
	//	alice.connected.wait_for_ms(true, 40000).unwrap();
	//	bob.connected.wait_for_ms(true, 40000).unwrap();
	alice.connected.wait_for(true).unwrap();
	bob.connected.wait_for(true).unwrap();


	let n_iter = 100;

	thread::spawn(move || {
		let mut count = 0;
		let buf = [0; 10*1024];

		for _ in 0..n_iter {
			let mut pos = 0;
			while pos < buf.len() {
				match alice.write(&buf[pos..]) {
					Ok(len) => {
						count += len;
						pos += len;
					},
					Err(error) => {
						if error.kind() == io::ErrorKind::WouldBlock {
							continue
						}
						panic!(error);
					}
				}
			}
		}

		debug!("alice.close()");
		alice.close(false);
		info!("alice done and sent {} bytes.", count);
	});

	let mut count = 0;
	let mut buf = vec![0; 10*1024];

	let mut is_connected = true;
	while is_connected {
		match bob.read(&mut buf[..]) {
			Ok(len)  => count += len,
			Err(err) => {
				debug!("bob.read = {:?}", err);

				match err.kind() {
					io::ErrorKind::WouldBlock => continue,
					io::ErrorKind::NotConnected => is_connected = false,
					_ => panic!(err),
				}

				thread::sleep_ms(100);
			}
		}
		assert!(buf.iter().all(|x| *x == 0x0));
	}

	assert_eq!(count, n_iter*buf.len());
}
