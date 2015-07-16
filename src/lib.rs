#![allow(dead_code)]

#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate log;
extern crate libc;
extern crate env_logger;

mod bindings;
mod api;
mod condition_variable;
mod pseudo_tcp_socket;

use std::io::{Result,Error};
use std::sync::mpsc::{Sender, Receiver};
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use std::thread::sleep_ms;
use std::mem;
use std::ptr;
use std::boxed::Box;
use libc::consts::os::posix88::EWOULDBLOCK;
use std::ffi::CString;

use bindings as ffi;

fn set_mutex<T>(var:   &Arc<(Mutex<T>,Condvar)>,
                value: T)
{
	let &(ref lock, ref cvar) = &**var;
	let mut guard = lock.lock().unwrap();

	*guard = value;
	cvar.notify_all();
}

extern fn on_closed(_:         *mut libc::c_int,
                    _:         libc::c_int,
                    user_data: *mut UserData)
{
	info!("on_closed()");
	let tx = unsafe {
		&mut (*user_data).stream_tx
	};
	let connected = unsafe {
		&(*user_data).connected
	};

	*tx = None;
	set_mutex(connected, false);
}

extern fn on_readable(ptr:       *mut bindings::_PseudoTcpSocket,
                      user_data: *mut UserData)
{
	debug!("on_readable()");

	// we don't use the user_data ptr here because it's locked in
	// the pseudo_tcp_socket_notify_packet block
	let stream_tx = unsafe { &(*user_data).stream_tx };

	while let &Some(ref tx) = stream_tx
	{
		let mut buf = vec![0u8; 64*1024];
		let buf_ptr = buf.as_mut_ptr() as *mut i8;
		let buf_len = buf.len() as libc::c_int;

		let len = unsafe {
			ffi::pseudo_tcp_socket_recv(ptr, buf_ptr, buf_len)
		};
		
		if len < 0 {
			warn!("on_readable(): len={}", len);
			break;
		}

		buf.truncate(len as usize);
		
		if let Err(err) = tx.send(buf) {
			warn!("on_readable(): send failed: {:?}", err);
			break;
		}
	}
}

extern fn on_connected(ptr: *mut libc::c_int, user_data: *mut UserData) {
	debug!("on_connected {:?}!", ptr);
	let connected = unsafe {
		&(*user_data).connected
	};

	set_mutex(connected, true);
}

extern "C" fn send(_:         *mut libc::c_int,
                   ptr:       *mut u8,
                   len:       libc::c_int,
                   user_data: *mut UserData)
	-> libc::c_uint
{
	debug!("send callback");

	let buf = unsafe {
		std::slice::from_raw_parts(ptr, len as usize)
		           .to_vec()
	};

	let ref tx = unsafe { &(*user_data).dgram_tx };

	match tx.send(buf) {
		Ok(_)    => ffi::WR_SUCCESS.bits(),
		Err(err) => {
			warn!("Sent failed: {:?}", err);
			ffi::WR_FAIL.bits()
		},
	}
}

struct UserData {
	ptr:        Arc<Mutex<*mut ffi::_PseudoTcpSocket>>,
	dgram_tx:   Sender<Vec<u8>>,
	stream_tx:  Option<Sender<Vec<u8>>>,
	connected:  Arc<(Mutex<bool>,Condvar)>,
}

impl Drop for UserData {
	fn drop(&mut self) {
		info!("UserData::drop");
	}
}

pub struct PseudoTcpStream {
	ptr:       Arc<Mutex<*mut ffi::_PseudoTcpSocket>>,
	user_data: Arc<Box<UserData>>,
	connected: Arc<(Mutex<bool>,Condvar)>,
	callbacks: Arc<ffi::PseudoTcpCallbacks>,
}

impl PseudoTcpStream {
	pub fn new_from(dgram_tx:  Sender<Vec<u8>>, dgram_rx:  Receiver<Vec<u8>>,
	                stream_tx: Sender<Vec<u8>>, stream_rx: Receiver<Vec<u8>>)
		-> PseudoTcpStream
	{
		let ptr       = Arc::new(Mutex::new(ptr::null_mut()));
		let connected = Arc::new((Mutex::new(false), Condvar::new()));

		let x = UserData {
			ptr:       ptr.clone(),
			dgram_tx:  dgram_tx,
			stream_tx: Some(stream_tx),
			connected: connected.clone()
		};
		let mut user_data = Box::new(x);

		let mut callbacks = unsafe { 
			ffi::PseudoTcpCallbacks {
				user_data:         mem::transmute(&mut *user_data),
				PseudoTcpOpened:   Some(mem::transmute(on_connected)),
				PseudoTcpReadable: Some(mem::transmute(on_readable)),
				PseudoTcpWritable: None,
				PseudoTcpClosed:   Some(mem::transmute(on_closed)),
				WritePacket:       Some(mem::transmute(send)),
			}
		};

//		mem::forget(user_data);

		let conversation = 1;
		let raw_ptr = unsafe {
			ffi::pseudo_tcp_socket_new(conversation, &mut callbacks)
		};
		assert!(!raw_ptr.is_null());
		*(ptr.lock().unwrap()) = raw_ptr;

		let stream = PseudoTcpStream {
			ptr:       ptr.clone(),
			connected: connected.clone(),
			user_data: Arc::new(user_data),
			callbacks: Arc::new(callbacks),
		};

		let this = stream.clone();
		Self::spawn_clock(this).unwrap();

		let this = stream.clone();
		Self::spawn_read_dgram(this, dgram_rx).unwrap();

		let this = stream.clone();
		Self::spawn_read_stream(this, stream_rx).unwrap();

		stream
	}

	fn spawn_clock(this: PseudoTcpStream) -> Result<thread::JoinHandle<()>> {
		let name = "PseudoTcpStream clock".to_string();

		thread::Builder::new().name(name).spawn(move || {
			loop {
				let mut timeout_ms: libc::c_ulong = 0;

				let res = unsafe {
					let ptr = this.ptr.lock().unwrap();
					ffi::pseudo_tcp_socket_get_next_clock(*ptr, &mut timeout_ms)
				};

				let socket_is_closed = (res == ffi::FALSE);
				if socket_is_closed {
					break;
				}

				let now = unsafe {
					(ffi::g_get_monotonic_time() / 1000) as u64
				};
				timeout_ms -= now;

				debug!("sleeping for {}ms", timeout_ms);
				sleep_ms(timeout_ms as u32);

				unsafe {
					let ptr = this.ptr.lock().unwrap();
					ffi::pseudo_tcp_socket_notify_clock(*ptr)
				};
			}
		})
	}


	fn spawn_read_dgram(this: PseudoTcpStream, dgram_rx: Receiver<Vec<u8>>)
		-> Result<thread::JoinHandle<()>>
	{
		let name = "PseudoTcpStream rx dgram".to_string();

		thread::Builder::new().name(name).spawn(move || {
			for buf in dgram_rx.iter() {
				let buf_ptr = buf.as_ptr() as *const i32;
				let buf_len = buf.len() as libc::c_int;

				let res = unsafe {
					let ptr = this.ptr.lock().unwrap();
					ffi::pseudo_tcp_socket_notify_packet(*ptr, buf_ptr, buf_len)
				};

				assert!(res != ffi::FALSE);
			}

			warn!("fin: dgram_rx closed!");
			unsafe {
				let ptr = this.ptr.lock().unwrap();
				ffi::pseudo_tcp_socket_close(*ptr, ffi::TRUE)
			};
		})
	}

	fn spawn_read_stream(this: PseudoTcpStream, stream_rx: Receiver<Vec<u8>>)
		-> Result<thread::JoinHandle<()>>
	{
		thread::Builder::new().name("PseudoTcpStream rx stream".to_string()).spawn(move || {
			this.wait_for_connection();

			for buf in stream_rx.iter().take_while(|_| this.is_connected()) {
				debug!("rx-stream: got len={} on {:?}", buf.len() as libc::c_int, this.ptr);

				while this.is_connected() {
					let buf_ptr = buf.as_ptr() as *const i8;
					let buf_len = buf.len() as libc::c_int;

					let len = unsafe {
						let ptr = this.ptr.lock().unwrap();
						ffi::pseudo_tcp_socket_send(*ptr, buf_ptr, buf_len)
					};

					if len >= buf_len {
						if (len as usize) != buf.len() {
							warn!("only sent {} of {} bytes", len, buf.len());
						}

						break
					}
	
					let err = this.get_last_error();
					if err.raw_os_error() != Some(EWOULDBLOCK) {
						error!("{:?}", err);
						set_mutex(&this.connected, false);
					} else {
						info!("error EWOULDBLOCK sending buffer, retrying...");
						break
					}
				}
			}

			warn!("fin: stream_rx closed!");
			unsafe {
				let ptr = this.ptr.lock().unwrap();
				ffi::pseudo_tcp_socket_close(*ptr, ffi::TRUE)
			};
		})
	}

	pub fn connect(&self) -> Result<()> {
		let res = unsafe {
			let ptr = self.ptr.lock().unwrap();
			ffi::pseudo_tcp_socket_connect(*ptr)
		};
	
		if res != ffi::FALSE {
			self.wait_for_connection(); // TODO wait for infinity?
			Ok(())
		} else {
			Err(self.get_last_error())
		}
	}

	fn is_connected(&self) -> bool {
			let &(ref lock, _) = &*self.connected;
			let connected = lock.lock().unwrap();
			*connected
	}

	fn wait_for_connection(&self) {
			let &(ref lock, ref cvar) = &*self.connected;
			let mut connected = lock.lock().unwrap();
			while !*connected {
				connected = cvar.wait(connected).unwrap();
			}
	}

	pub fn set_mtu(&self, mtu: u16) {
		let ptr = self.ptr.lock().unwrap();
		unsafe {
			ffi::pseudo_tcp_socket_notify_mtu(*ptr, mtu as libc::c_int)
		}
	}

	pub fn set_no_delay(&self, val: bool) {
		//unimplemented!();
		
		let guard = self.ptr.lock().unwrap();
		let ptr = (*guard) as *const libc::c_void;

		let name = &b"no-delay"[..];
		let prop = CString::new(name).unwrap();
		
		let gval = if val {!ffi::FALSE} else {ffi::FALSE};

		unsafe {
			ffi::g_object_set(ptr, prop.as_ptr(), gval, std::ptr::null())
		}
	}

	pub fn force_close(self) {
		let ptr = self.ptr.lock().unwrap();
		unsafe {
			ffi::pseudo_tcp_socket_close(*ptr, ffi::TRUE)
		}
	}

	pub fn close(&self) {
		let ptr = self.ptr.lock().unwrap();
		unsafe {
			ffi::pseudo_tcp_socket_close(*ptr, ffi::FALSE)
		}
	}

	fn get_last_error(&self) -> Error {
		let ptr = self.ptr.lock().unwrap();
		let code = unsafe {
			ffi::pseudo_tcp_socket_get_error(*ptr)
		};
		Error::from_raw_os_error(code)
	}
}

unsafe impl Send for PseudoTcpStream {}

impl Drop for PseudoTcpStream {
	fn drop(&mut self) {
		let ptr = self.ptr.lock().unwrap();
		unsafe {
			ffi::g_object_unref(*ptr);
		}
	}
}

impl Clone for PseudoTcpStream {
	fn clone(&self) -> PseudoTcpStream {
		let stream = PseudoTcpStream {
			ptr:       self.ptr.clone(),
			user_data: self.user_data.clone(),
			connected: self.connected.clone(),
			callbacks: self.callbacks.clone(),
		};

		let ptr = self.ptr.lock().unwrap();
		unsafe {
			ffi::g_object_ref(*ptr)
		};

		stream
	}
}
/*
#[cfg(test)]
mod test {
	use std::thread;
	use std::sync::mpsc::channel;

	use bindings as ffi;

	#[test]
	fn test() {
		super::env_logger::init().unwrap();

		unsafe {
			ffi::g_type_init()
		};

		let (dgram_tx_a, dgram_rx_b) = channel();
		let (dgram_tx_b, dgram_rx_a) = channel();
		let (left_stream_tx_a, left_stream_rx_b) = channel();
		let (left_stream_tx_b, left_stream_rx_a) = channel();
		let (right_stream_tx_a, right_stream_rx_b) = channel();
		let (right_stream_tx_b, right_stream_rx_a) = channel();

		let left_tx = left_stream_tx_b;
		let right_rx = right_stream_rx_b;

		let left = super::PseudoTcpStream::new_from(dgram_tx_a, dgram_rx_a, left_stream_tx_a, left_stream_rx_a);
		let right = super::PseudoTcpStream::new_from(dgram_tx_b, dgram_rx_b, right_stream_tx_a, right_stream_rx_a);

		left.set_mtu(1500);
		right.set_mtu(1500);

		left.set_no_delay(true);
		right.set_no_delay(false);

		thread::spawn(move || {
			left.connect().unwrap();
			info!("connected!");

			for i in 0..150 as i64 {
				let v = vec![i as u8; 512];

				left_tx.send(v).unwrap();
				thread::sleep_ms(100);
			}
			left.close();
			info!("all written!");
		});

		let mut i: i64 = 0;
		while i < 150*512 {
			let buf = right_rx.recv().unwrap();
			info!("test got len={}", buf.len());

			for v in buf.iter() {
				assert_eq!(*v, (i/512) as u8);
				i = i+1;
			}
			info!("i={}", i);
		}
	}
}
*/