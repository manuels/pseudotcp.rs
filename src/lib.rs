#![allow(dead_code)]

#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate log;
extern crate libc;
extern crate env_logger;

mod bindings;

use std::io::{Result,Error};
use std::io::{Read,Write};
use std::sync::mpsc::{Sender, Receiver};
use std::sync::{Arc, Mutex, Condvar};
use std::thread;
use std::thread::sleep_ms;
use std::mem;
use std::boxed::Box;

use bindings as ffi;

//extern fn on_connected(tcp: *mut libc::c_int, user_data: *mut libc::c_int) {
extern fn on_connected(_: *mut libc::c_int, user_data: *mut libc::c_int) {
	debug!("on_connected!");
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
	unsafe {
		let tx:*mut UserData = mem::transmute(user_data);

		let buf = Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize);

		(*tx).tx.send(buf.clone()).unwrap();
		debug!("sent len={}", buf.len());

		mem::forget(tx);
		mem::forget(buf);

		return ffi::WR_SUCCESS.bits();
	}
}

struct UserData {
	tx: Sender<Vec<u8>>,
	connected: Arc<(Mutex<bool>,Condvar)>,
}

impl Drop for UserData {
	fn drop(&mut self) {
		info!("UserData::drop");
	}
}

struct PseudoTcpStream {
	ptr:       Arc<Mutex<*mut ffi::_PseudoTcpSocket>>,
	connected: Arc<(Mutex<bool>,Condvar)>,
	callbacks: Arc<ffi::PseudoTcpCallbacks>,
}

impl PseudoTcpStream {
	pub fn new_from(tx: Sender<Vec<u8>>, rx: Receiver<Vec<u8>>) -> PseudoTcpStream {
		let connected = Arc::new((Mutex::new(false), Condvar::new()));
		let x = UserData {
			tx:        tx,
			connected: connected.clone()
		};
		let mut user_data = Box::new(x);

		let conversation = 1;
		let mut callbacks = ffi::PseudoTcpCallbacks {
			user_data:         unsafe { mem::transmute(&mut *user_data) },
			PseudoTcpOpened:   Some(on_connected),
			PseudoTcpReadable: None,
			PseudoTcpWritable: None,
			PseudoTcpClosed:   None,
			WritePacket:       Some(send),
		};

		unsafe {mem::forget(user_data)};

		let ptr = unsafe {
			ffi::pseudo_tcp_socket_new(conversation, &mut callbacks)
		};
		unsafe {
			ffi::pseudo_tcp_set_debug_level(ffi::PSEUDO_TCP_DEBUG_VERBOSE.bits());
		}

		assert!(!ptr.is_null());

		let stream = PseudoTcpStream {
			ptr:       Arc::new(Mutex::new(ptr)),
			connected: connected.clone(),
			callbacks: Arc::new(callbacks),
		};

		let this = stream.clone();
		thread::Builder::new().name("PseudoTcpStream clock".to_string()).spawn(move || {
			loop {
				let ptr = this.ptr.lock().unwrap();
				let mut timeout_ms: libc::c_long = 0;
				let res = unsafe {
					ffi::pseudo_tcp_socket_get_next_clock(*ptr, &mut timeout_ms)
				};
				if res == ffi::FALSE {
					// socket was closed
					break;
				}
				drop(ptr);

				debug!("sleeping for {}ms", timeout_ms);
				sleep_ms(timeout_ms as u32);

				let ptr = this.ptr.lock().unwrap();
				unsafe {
					ffi::pseudo_tcp_socket_notify_clock(*ptr)
				};
			}
		}).unwrap();

		let this = stream.clone();
		thread::Builder::new().name("PseudoTcpStream rx-to-tcp".to_string()).spawn(move || {
			for buf in rx.iter() {
				debug!("rx-to-tcp: got len={} on {:?}", buf.len() as libc::c_int, this.ptr);
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

impl Read for PseudoTcpStream {
	fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
		let ptr = self.ptr.lock().unwrap();
		let len = unsafe {
			ffi::pseudo_tcp_socket_recv(*ptr,
				buf.as_mut_ptr() as *mut i8,
				buf.len() as libc::c_int)
		};
		drop(ptr);

		if len < 0 {
			Err(self.get_last_error())
		} else {
			Ok(len as usize)
		}
	}
}

impl Write for PseudoTcpStream {
	fn write(&mut self, buf: &[u8]) -> Result<usize> {
		let ptr = self.ptr.lock().unwrap();
		let len = unsafe {
			ffi::pseudo_tcp_socket_send(*ptr,
				buf.as_ptr() as *const i8,
				buf.len() as libc::c_int)
		};
		drop(ptr);

		if len < 0 {
			Err(self.get_last_error())
		} else {
			Ok(len as usize)
		}
	}

	fn flush(&mut self) -> Result<()> {
		/* noop */
		Ok(())

		/*
		maybe something like
			set_mtu(0);
			pseudo_tcp_socket_notify_clock();
		or
			set_mtu(0);
			pseudo_tcp_socket_send(buf.len=0)
		works?
		*/
	}
}

#[cfg(test)]
mod test {
	use std::io::{Read,Write};
	use std::io::ErrorKind;
	use std::thread;
	use std::sync::mpsc::channel;

	use bindings as ffi;

	#[test]
	fn test() {
		super::env_logger::init().unwrap();

		unsafe {
			ffi::g_type_init()
		};

		let (tx_a, rx_b) = channel();
		let (tx_b, rx_a) = channel();

		let mut left  = super::PseudoTcpStream::new_from(tx_a, rx_a);
		info!("new() done");
		let mut right = super::PseudoTcpStream::new_from(tx_b, rx_b);
		info!("new() done");

		left.set_mtu(1500);
		right.set_mtu(1500);

		thread::spawn(move || {
			left.connect().unwrap();
			info!("connected!");

			for _ in 0..10 {
				for i in 0..1024*1024 as i64 {
					let v = vec![i as u8];

					loop {
						match left.write(&v[..]) {
							Ok(_) => break,
							Err(e) => {
								if e.kind() == ErrorKind::WouldBlock {
									continue
								} else {
									panic!("kind={:?}, desc={:?}", e.kind(), e)
								}
							},
						}
					}
				}
				info!("about to flush!");
				left.flush().unwrap();
			}
			left.close();
			info!("all written!");
		});

		let mut i: i64 = 0;
		for _ in 0..10 {
			info!("i counter reset.");

			let mut buf = vec![0; 1024*1024];

			let mut len;
			loop {
				len = match right.read(&mut buf[..]) {
					Ok(len) => len,
					Err(e) => {
						if e.kind() == ErrorKind::WouldBlock {
							continue
						} else {
							panic!("kind={:?}, desc={:?}", e.kind(), e)
						}
					},
				};
				break
			}

			buf.truncate(len);
			info!("test got len={}", buf.len());

			for v in buf.iter() {
				assert_eq!(*v, i as u8);
				i = (i+1) % (1024*1024);
			}
			info!("i={}", i);
		}
	}
}