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

use bindings as ffi;

extern fn on_closed(_: *mut libc::c_int, _: libc::c_int, user_data: *mut libc::c_int) {
	info!("on_closed()");
	unsafe {
		let user_data:*mut UserData = mem::transmute(user_data);

		(*user_data).stream_tx = None;
	};
}

extern fn on_readable(ptr: *mut libc::c_int, user_data: *mut libc::c_int) {
	info!("on_readable()");
	unsafe {
		let user_data:*mut UserData = mem::transmute(user_data);

		// we don't use the user_data ptr here because it's locked in
		// the pseudo_tcp_socket_notify_packet block
		//let ptr = (*user_data).ptr.lock().unwrap();

		loop {
			let mut buf = vec![0u8; 64*1024];
			let len = ffi::pseudo_tcp_socket_recv(ptr as *mut bindings::_PseudoTcpSocket,
			                                      buf.as_mut_ptr() as *mut i8,
			                                      buf.len() as libc::c_int);
			
			info!("on_readable(): len={}", len);
			if len >= 0 {
				buf.truncate(len as usize);
				match (*user_data).stream_tx {
					Some(ref tx) => tx.send(buf).unwrap(),
					None => break, /* stream already closed */
				}
			} else {
				break;
			}
		}
		drop(ptr);
		
		mem::forget(user_data);
	}
}

extern fn on_connected(ptr: *mut libc::c_int, user_data: *mut libc::c_int) {
	debug!("on_connected {:?}!", ptr);
	unsafe {
		let user_data:*mut UserData = mem::transmute(user_data);

		let &(ref lock, ref cvar) = &*(*user_data).connected;
		let mut connected = lock.lock().unwrap();
		*connected = true;
		cvar.notify_one();

		mem::forget(user_data);
	}
}

//extern "C" fn send(tcp:       *mut libc::c_int,
//                   ptr:       *const libc::c_int,
//                   len:       libc::c_int,
//                   user_data: *mut libc::c_int)
extern "C" fn send(_:         *mut libc::c_int,
                   ptr:       *const libc::c_int,
                   len:       libc::c_int,
                   user_data: *mut libc::c_int)
	-> libc::c_uint
{
	debug!("send callback");
	unsafe {
		let tx:*mut UserData = mem::transmute(user_data);

		let buf = Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize);

		(*tx).dgram_tx.send(buf.clone()).unwrap();
		debug!("sent len={}", buf.len());

		mem::forget(tx);
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
		let ptr = Arc::new(Mutex::new(ptr::null_mut()));
		let connected = Arc::new((Mutex::new(false), Condvar::new()));

		let x = UserData {
			ptr:       ptr.clone(),
			dgram_tx:  dgram_tx,
			stream_tx: Some(stream_tx),
			connected: connected.clone()
		};
		let mut user_data = Box::new(x);

		let conversation = 1;
		let mut callbacks = ffi::PseudoTcpCallbacks {
			user_data:         unsafe { mem::transmute(&mut *user_data) },
			PseudoTcpOpened:   Some(on_connected),
			PseudoTcpReadable: Some(on_readable),
			PseudoTcpWritable: None,
			PseudoTcpClosed:   Some(on_closed),
			WritePacket:       Some(send),
		};

		unsafe {mem::forget(user_data)};

		*(ptr.lock().unwrap()) = unsafe {
			let ptr = ffi::pseudo_tcp_socket_new(conversation, &mut callbacks);
			assert!(!ptr.is_null());

			ffi::pseudo_tcp_set_debug_level(ffi::PSEUDO_TCP_DEBUG_VERBOSE.bits());

			ptr
		};


		let stream = PseudoTcpStream {
			ptr:       ptr.clone(),
			connected: connected.clone(),
			callbacks: Arc::new(callbacks),
		};

		let this = stream.clone();
		thread::Builder::new().name("PseudoTcpStream clock".to_string()).spawn(move || {
			loop {
				let ptr = this.ptr.lock().unwrap();
				let mut timeout_ms: libc::c_ulong = 0;
				let res = unsafe {
					ffi::pseudo_tcp_socket_get_next_clock(*ptr, &mut timeout_ms)
				};
				if res == ffi::FALSE {
					// socket was closed
					break;
				}
				drop(ptr);

				let now = unsafe {
					(ffi::g_get_monotonic_time() / 1000) as u64
				};
				timeout_ms -= now;

				debug!("sleeping for {}ms", timeout_ms);
				sleep_ms(timeout_ms as u32);

				let ptr = this.ptr.lock().unwrap();
				unsafe {
					ffi::pseudo_tcp_socket_notify_clock(*ptr)
				};
			}
		}).unwrap();

		let this = stream.clone();
		thread::Builder::new().name("PseudoTcpStream rx dgram".to_string()).spawn(move || {
			for buf in dgram_rx.iter() {
				debug!("rx-dgram: got len={} on {:?}", buf.len() as libc::c_int, this.ptr);
				let ptr = this.ptr.lock().unwrap();
				let res = unsafe {
					ffi::pseudo_tcp_socket_notify_packet(*ptr,
						buf.as_ptr() as *const i32,
						buf.len() as libc::c_int)
				};
				drop(ptr);

				assert!(res != ffi::FALSE);
			}
			warn!("fin!");
		}).unwrap();

		let this = stream.clone();
		thread::Builder::new().name("PseudoTcpStream rx stream".to_string()).spawn(move || {
			for buf in stream_rx.iter() {
				loop {
					debug!("rx-stream: got len={} on {:?}", buf.len() as libc::c_int, this.ptr);
					let ptr = this.ptr.lock().unwrap();
					let len = unsafe {
						ffi::pseudo_tcp_socket_send(*ptr,
							buf.as_ptr() as *const i8,
							buf.len() as libc::c_int)
					};
					drop(ptr);

					if len == -1 {
						let err = this.get_last_error();
						const EAGAIN: i32 = 11;
						if err.raw_os_error().unwrap() == EAGAIN {
							info!("error EAGAIN sending buffer, retrying...");
							continue
						} else {
							panic!("{:?}", err);
						}
					}

					if (len as usize) != buf.len() {
						warn!("only sent {} of {} bytes", len, buf.len());
					}
					break;
				}
			}
			warn!("fin!");
		}).unwrap();

		stream
	}

	pub fn connect(&self) -> Result<()> {
		let ptr = self.ptr.lock().unwrap();
		let res = unsafe {
			ffi::pseudo_tcp_socket_connect(*ptr)
		};
		drop(ptr);
	
		if res != ffi::FALSE {
			let &(ref lock, ref cvar) = &*self.connected;
			let mut connected = lock.lock().unwrap();
			while !*connected {
				connected = cvar.wait(connected).unwrap();
			}
			Ok(())
		} else {
			Err(self.get_last_error())
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
		self.close();

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

			for i in 0..10 as i64 {
				let v = vec![i as u8; 1];

				left_tx.send(v).unwrap();
			}
			left.close();
			info!("all written!");
		});

		let mut i: i64 = 0;
		while i < 10 {
			let buf = right_rx.recv().unwrap();
			info!("test got len={}", buf.len());

			for v in buf.iter() {
				assert_eq!(*v, i as u8);
				i = i+1;
			}
			info!("i={}", i);
		}
	}
}
