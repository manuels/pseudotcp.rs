use std::io;
use std::io::{Read, Write};
use std::io::ErrorKind::{NotConnected, WouldBlock};
use std::thread;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Sender, Receiver, channel};

use libc::consts::os::posix88::EAGAIN;

use api;
use bindings as ffi;
use condition_variable::{ConditionVariable, Notify};

const CONVERSATION_ID:i32 = 0xFEED;
const FIN:[u8;2] = [0xDE, 0xAD];

pub struct PseudoTcpSocket;
pub struct PseudoTcpChannel;

#[derive(Clone,PartialEq,Debug)]
enum ConnectState {
	NotConnected,
	Connected,
	WasConnected,
}

pub struct PseudoTcpStream {
	dbg:           String,
	tcp:           Arc<Mutex<api::PseudoTcpSocket>>,
	connected:     Arc<ConditionVariable<ConnectState>>,
	readable:      Arc<ConditionVariable<bool>>,
	writable:      Arc<ConditionVariable<bool>>,
	notify_clock:  Arc<ConditionVariable<()>>,
}

impl PseudoTcpSocket {
	/// waits for a connection in the background
	pub fn listen(ch: (Sender<Vec<u8>>, Receiver<Vec<u8>>), dbg: String) -> PseudoTcpStream {
		PseudoTcpStream::new(ch, dbg)
	}

	pub fn connect(ch: (Sender<Vec<u8>>, Receiver<Vec<u8>>), dbg: String) -> io::Result<PseudoTcpStream> {
		let stream = PseudoTcpStream::new(ch, dbg);
		
		stream.connect().map(|_| stream)
	}
}

impl PseudoTcpStream {
	fn new(ch: (Sender<Vec<u8>>, Receiver<Vec<u8>>), dbg: String) -> PseudoTcpStream {
		let (tx, rx) = ch;

		let connected = Arc::new(ConditionVariable::new(ConnectState::NotConnected));
		let readable  = Arc::new(ConditionVariable::new(false));
		let writable  = Arc::new(ConditionVariable::new(false));

		let debug = dbg.clone();
		let callbacks = {
			let tx         = tx.clone();
			let readable   = readable.clone();
			let writable   = writable.clone();
			let connected1 = connected.clone();
			let connected2 = connected.clone();

			let write_packet = move |buf: &[u8]| {
				let res = tx.send(buf.to_vec());
				if let Err(err) = res {
					error!("{} write_packet: {:?}", debug, err);
					ffi::WR_FAIL
				} else {
					ffi::WR_SUCCESS
				}
			};

			api::Callbacks {
				opened:       Some(Box::new(move ||  connected1.set(ConnectState::Connected, Notify::All))),
				aborted:      Some(Box::new(move |_| connected2.set(ConnectState::WasConnected, Notify::All))),
				readable:     Some(Box::new(move ||  readable.set(true, Notify::All))),
				writable:     Some(Box::new(move ||  writable.set(true, Notify::All))),
				write_packet: Some(Box::new(write_packet)),
			}
		};

		let tcp = api::PseudoTcpSocket::new(CONVERSATION_ID, callbacks);
		let tcp = Arc::new(Mutex::new(tcp));
		let notify_clock = Arc::new(ConditionVariable::new(()));

		let socket = PseudoTcpStream {
			dbg:           dbg,
			tcp:           tcp,
			readable:      readable,
			writable:      writable,
			connected:     connected,
			notify_clock: notify_clock,
		};

		socket.spawn_udp_receiver(rx);
		socket.spawn_clock(tx);

		socket
	}

	fn connect(&self) -> io::Result<()> {
		let res = {
			let lock = self.tcp.lock().unwrap();
			lock.connect()
		};

		self.adjust_clock();
		res
	}

	/// use this for the listening socket
	pub fn wait_for_connection(&self, timeout_ms: i64) -> bool
	{
		self.connected.wait_for_ms(ConnectState::Connected, timeout_ms).unwrap()
	}

	pub fn close(&self, force: bool) {
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
		Self::do_adjust_clock(&self.notify_clock);
	}

	fn do_adjust_clock(notify_clock:  &Arc<ConditionVariable<()>>) {
		(*notify_clock).set((), Notify::All)
	}

	fn spawn_clock(&self, tx: Sender<Vec<u8>>) -> thread::JoinHandle<()> {
		let notify_clock = self.notify_clock.clone();
		let connected    = self.connected.clone();
		let readable     = self.readable.clone();
		let tcp = self.tcp.clone();

		let dbg = self.dbg.clone();
		thread::spawn(move || {
			let mut timer = Some(0);

			while let Some(timeout_ms) = timer {
				warn!("{} clock: {:?}", dbg, timeout_ms);
				(*notify_clock).wait_ms(timeout_ms as u32).unwrap();

				timer = {
					let lock = tcp.lock().unwrap();

					lock.notify_clock();
					lock.get_next_clock_ms()
				};

				if connected.get().unwrap() == ConnectState::WasConnected {
					break
				}
			}
			/* socket was closed. (see pseudo_tcp_socket_close() docs) */

			// apparently libnice v0.0.11 does not take care of telling the
			// other peer that the connection was closed.
			// This is fixed in the latest libnice version!
			// We do this by sending FIN (0xDEAD) over the wire. Usual
			// libnice packets begin with the conversation id (0xFEED).
			let _ = tx.send(FIN.to_vec());
			thread::sleep_ms(500);

			error!("{} spawn_clock: tcp socket: finished closing socket.", dbg);
			readable.touch(Notify::All);
			connected.set(ConnectState::WasConnected, Notify::All);
		})
	}

	fn spawn_udp_receiver(&self, rx: Receiver<Vec<u8>>) -> thread::JoinHandle<()> {
		let tcp = self.tcp.clone();
		let connected = self.connected.clone();
		let notify_clock = self.notify_clock.clone();

		let dbg = self.dbg.clone();
		thread::spawn(move || {
			for buf in rx {
				let lock = tcp.lock().unwrap();

				// Usual libnice packets begin with the conversation id which is
				// 0xFEED in our case. (see PseudoTcpStream::spawn_clock())
				if buf == FIN {
					info!("Got FIN!");
					lock.close(false);

					break
				} else {
					let res = lock.notify_packet(&buf[..]);
					if !res {
						warn!("notify_packet failed!");
					}
				}

				// the next line is the same as self.adjust_clock():
				Self::do_adjust_clock(&notify_clock);
			}

			let lock = tcp.lock().unwrap();
			lock.close(false);
			error!("{} spawn_udp_receiver: Socket was closed from remote!", dbg);
			connected.set(ConnectState::WasConnected, Notify::All);
		})
	}

	fn recv(&self, buf: &mut [u8]) -> io::Result<usize>
	{
		// do NOT wait for self.readable here: if the connection is closed,
		// we will never satisfy it!
		// self.readable.wait_for(true).unwrap();

		let res = {
			let lock = self.tcp.lock().unwrap();
			let res = lock.recv(buf);

			if let Err(err) = res {
				if err.raw_os_error() == Some(EAGAIN) {
					// We must keep the lock to ensure that readable callback
					// is not called before we set self.readable to false!
					self.readable.set(false, Notify::All);
				} else {
					debug!("pseudo_tcp_socket_recv()=={:?}", err);
				}
				Err(err)
			} else {
				res
			}
		};

		self.adjust_clock();

		let disconnected = match res {
			Ok(len) if len == 0 => true,
			Err(ref e) if e.kind() == WouldBlock => {
				self.connected.get().unwrap() != ConnectState::Connected
			},
			_ => false,
		};

		if disconnected {
			let error = io::Error::new(NotConnected, "");
			return Err(error);
		}

		res
	}

	fn send(&self, buf: &[u8]) -> io::Result<usize>
	{
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
				Err(ref err) if err.kind() == WouldBlock => {
					self.writable.set(false, Notify::All);
				}
				_ => (),
			}

			res
		};
		self.adjust_clock();

		res
	}

	pub fn to_channel(self) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
		PseudoTcpChannel::new(self)
	}

	pub fn is_connected(&self) -> bool {
		self.connected.get().unwrap() == ConnectState::Connected
	}
}

impl io::Read for PseudoTcpStream {
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		self.recv(buf)
	}
}

impl io::Write for PseudoTcpStream {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.send(buf)
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(()) //TODO (is that even possible?)
	}
}

impl PseudoTcpChannel {
	fn new(tcp: PseudoTcpStream) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
		let (tx_a, rx_b) = channel();
		let (tx_b, rx_a) = channel();

		let tcp = Arc::new(tcp);
		Self::spawn_tcp_sender(&tcp, rx_a);
		Self::spawn_tcp_receiver(&tcp, tx_a);

		(tx_b, rx_b)
	}

	fn spawn_tcp_sender(tcp: &Arc<PseudoTcpStream>, rx: Receiver<Vec<u8>>) -> thread::JoinHandle<()>
	{
		let tcp = tcp.clone();

		let dbg = tcp.dbg.clone();
		thread::spawn(move || {
			tcp.connected.wait_for(ConnectState::Connected).unwrap();

			let mut is_connected = true;

			for buf in rx.iter().take_while(|_| tcp.is_connected()) {
				let mut pos = 0;
				while pos < buf.len() && is_connected {
					let res = tcp.send(&buf[pos..]);
					let err_kind = res.as_ref().err().map(|e| e.kind());

					match (res, err_kind) {
						(Ok(len), _) => pos += len,
						(Err(_), Some(NotConnected)) => is_connected = false,
						(Err(_), Some(WouldBlock)) => {
							tcp.writable.wait_for_condition(|w| {
								*w || !tcp.is_connected()
							});
						},
						(Err(err), _) => panic!("{:?}", err),
					}
				}

				if !is_connected {
					break
				}
			}

			error!("{} spawn_tcp_sender: Closing stream.", dbg);
			tcp.connected.set(ConnectState::WasConnected, Notify::All);
			tcp.close(false);
		})
	}

	fn spawn_tcp_receiver(tcp: &Arc<PseudoTcpStream>, tx: Sender<Vec<u8>>) -> thread::JoinHandle<()>
	{
		let tcp = tcp.clone();

		let dbg = tcp.dbg.clone();
		thread::spawn(move || {
			let mut buf = [0; 10*1024];

			// TODO:
			tcp.connected.wait_for(ConnectState::Connected).unwrap();

			// We cannot use tcp.connected here because there there might
			// pending notify_packets that were not processed by PseudoTcpSocket
			loop {
				warn!("Recv()'ing...");
				let res = tcp.recv(&mut buf);
				let err_kind = res.as_ref().err().map(|e| e.kind());
				warn!("Recv()'ed {:?}.", res);

				match (res, err_kind) {
					(Ok(len), _) => tx.send(buf[..len].to_vec()).unwrap(),
					(Err(_), Some(NotConnected)) => break,
					(Err(_), Some(WouldBlock)) => {
						warn!("readable.wait_for...");
						tcp.readable.wait_for_condition(|r| {
							warn!("readable.wait_for: {} {:?}", *r, tcp.connected.get().unwrap());
							*r || !tcp.is_connected()
						}).unwrap();

						debug!("readable.wait_for. done");
					},
					(Err(err), _) => error!("spawn_tcp_receiver {:?}", err),
				}
			}

			tcp.connected.set(ConnectState::WasConnected, Notify::All);
			error!("{} spawn_tcp_receiver: Peer disconnected.", dbg);
		})
	}
}
