#![allow(dead_code)]

#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate log;
extern crate libc;
extern crate env_logger;

mod bindings;

use std::io::{Result,Error};
use std::sync::mpsc::{Sender, Receiver};
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use std::thread::sleep_ms;
use std::mem;
use std::ptr;
use std::boxed::Box;
use libc::consts::os::posix88::EAGAIN;

use bindings as ffi;

extern fn on_closed(_: *mut libc::c_int, _: libc::c_int, user_data: *mut UserData) {
	info!("on_closed()");
	unsafe {
		(*user_data).stream_tx = None;

		let &(ref lock, ref cvar) = &*(*user_data).connected;
		let mut connected = lock.lock().unwrap();
		*connected = false;
		cvar.notify_all();
	};
}

extern fn on_readable(ptr: *mut libc::c_int, user_data: *mut UserData) {
	debug!("on_readable()");
	unsafe {
		// we don't use the user_data ptr here because it's locked in
		// the pseudo_tcp_socket_notify_packet block
		//let ptr = (*user_data).ptr.lock().unwrap();
		let ptr = ptr as *mut bindings::_PseudoTcpSocket;

		loop {
			let mut buf = vec![0u8; 64*1024];
			let buf_ptr = buf.as_mut_ptr() as *mut i8;
			let buf_len = buf.len() as libc::c_int;

			let len = ffi::pseudo_tcp_socket_recv(ptr, buf_ptr, buf_len);
			
			if len >= 0 {
				buf.truncate(len as usize);
				match (*user_data).stream_tx {
					Some(ref tx) => tx.send(buf).unwrap(),
					None => break, /* stream already closed */
				}
			} else {
				warn!("on_readable(): len={}", len);
				break;
			}
		}
		drop(ptr);
		
		mem::forget(user_data);
	}
}

extern fn on_connected(ptr: *mut libc::c_int, user_data: *mut UserData) {
	debug!("on_connected {:?}!", ptr);
	unsafe {
		let &(ref lock, ref cvar) = &*(*user_data).connected;
		let mut connected = lock.lock().unwrap();
		*connected = true;
		cvar.notify_all();

		mem::forget(user_data);
	}
}

extern "C" fn send(_:         *mut libc::c_int,
                   ptr:       *mut u8,
                   len:       libc::c_int,
                   user_data: *mut UserData)
	-> libc::c_uint
{
	debug!("send callback");
	unsafe {
		let ref tx = (*user_data).dgram_tx;
		let len    = len as usize;
		let buf    = Vec::from_raw_parts(ptr, len, len);

		let res = tx.send(buf.clone());

		if let Err(err) = res {
			warn!("Sent failed: {:?}", err);
			return ffi::WR_FAIL.bits();
		}

		mem::forget(user_data);
		mem::forget(buf);
	}

	return ffi::WR_SUCCESS.bits();
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

		let conversation = 1;
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

		mem::forget(user_data);

		*(ptr.lock().unwrap()) = unsafe {
			let ptr = ffi::pseudo_tcp_socket_new(conversation, &mut callbacks);
			assert!(!ptr.is_null());

			let level = ffi::PSEUDO_TCP_DEBUG_VERBOSE.bits();
			ffi::pseudo_tcp_set_debug_level(level);

			ptr
		};

		let stream = PseudoTcpStream {
			ptr:       ptr.clone(),
			connected: connected.clone(),
			callbacks: Arc::new(callbacks),
		};

		let this = stream.clone();
		Self::spawn_clock(this);

		let this = stream.clone();
		Self::spawn_read_dgram(this, dgram_rx);

		let this = stream.clone();
		Self::spawn_read_stream(this, stream_rx);

		stream
	}

	fn spawn_clock(this: PseudoTcpStream) {
		let name = "PseudoTcpStream clock".to_string();

		thread::Builder::new().name(name).spawn(move || {
			loop {
				let mut timeout_ms: libc::c_ulong = 0;

				let res = unsafe {
					let ptr = this.ptr.lock().unwrap();
					ffi::pseudo_tcp_socket_get_next_clock(*ptr, &mut timeout_ms)
				};
				if res == ffi::FALSE {
					// socket was closed
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
		}).unwrap();
	}


	fn spawn_read_dgram(this: PseudoTcpStream, dgram_rx: Receiver<Vec<u8>>) {
		thread::Builder::new().name("PseudoTcpStream rx dgram".to_string()).spawn(move || {
			for buf in dgram_rx.iter() {
				let buf_ptr = buf.as_ptr() as *const i32;
				let buf_len = buf.len() as libc::c_int;

				debug!("rx-dgram: got len={} on {:?} locking...", buf_len, this.ptr);
				let res = unsafe {
					let ptr = this.ptr.lock().unwrap();
					debug!("rx-dgram: locked.");

					ffi::pseudo_tcp_socket_notify_packet(*ptr, buf_ptr, buf_len)
				};
				debug!("rx-dgram: unlocked.");

				assert!(res != ffi::FALSE);
			}

			warn!("fin: dgram_rx closed!");
			unsafe {
				let ptr = this.ptr.lock().unwrap();
				ffi::pseudo_tcp_socket_close(*ptr, ffi::TRUE)
			};
		}).unwrap();
	}

	fn spawn_read_stream(this: PseudoTcpStream, stream_rx: Receiver<Vec<u8>>) {
		thread::Builder::new().name("PseudoTcpStream rx stream".to_string()).spawn(move || {
			this.wait_for_connection();

			for buf in stream_rx.iter() {
				loop {
					debug!("rx-stream: got len={} on {:?}", buf.len() as libc::c_int, this.ptr);
					let &(ref lock, _) = &*this.connected;
					let connected = lock.lock().unwrap();
					if !*connected {
						break
					}

					let buf_ptr = buf.as_ptr() as *const i8;
					let buf_len = buf.len() as libc::c_int;

					let len = unsafe {
						let ptr = this.ptr.lock().unwrap();
						ffi::pseudo_tcp_socket_send(*ptr, buf_ptr, buf_len)
					};
	
					if len == -1 {
						let err = this.get_last_error();
						
						if err.raw_os_error() == Some(EAGAIN) {
							info!("error EAGAIN sending buffer, retrying...");
							continue
						} else {
							error!("{:?}", err);
							
							let &(ref lock, ref cvar) = &*this.connected;
							let mut connected = lock.lock().unwrap();
							*connected = false;
							cvar.notify_all();
						}
					} else if (len as usize) != buf.len() {
						warn!("only sent {} of {} bytes", len, buf.len());
					}

					break;
				}
			}

			warn!("fin: stream_rx closed!");
			unsafe {
				let ptr = this.ptr.lock().unwrap();
				ffi::pseudo_tcp_socket_close(*ptr, ffi::TRUE)
			};
		}).unwrap();
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
