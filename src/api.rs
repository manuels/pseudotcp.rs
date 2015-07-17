// wrapper around C api

use std::io;
use std::mem;
use std::slice;
use std::fmt;

use bindings as ffi;

use libc::{c_int, c_uint, c_ulong, c_char, c_uchar};

pub struct PseudoTcpSocket {
	ptr:       *mut ffi::_PseudoTcpSocket,
	callbacks: Box<Callbacks>,
}

impl fmt::Debug for PseudoTcpSocket {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		write!(fmt, "{:?}", self.ptr)
	}

}

unsafe impl Send for PseudoTcpSocket {}

pub type OpenedCallback      = Fn();
pub type ReadableCallback    = Fn();
pub type WritableCallback    = Fn();
pub type AbortedCallback      = Fn(Option<io::Error>);
pub type WritePacketCallback = Fn(&[u8]) -> ffi::PseudoTcpWriteResult;

pub struct Callbacks {
	pub opened:       Option<Box<OpenedCallback>>,
	pub readable:     Option<Box<ReadableCallback>>,
	pub writable:     Option<Box<WritableCallback>>,
	pub aborted:      Option<Box<AbortedCallback>>,
	pub write_packet: Option<Box<WritePacketCallback>>,
}

extern "C" fn opened_callback(_:    *mut c_int,
                              data: *mut c_int)
{
	let user_data:&Callbacks = unsafe { mem::transmute(data) };

	user_data.opened.as_ref().map(|func| func());
}

extern "C" fn readable_callback(_:    *mut c_int,
                                data: *mut c_int)
{
	let user_data:&Callbacks = unsafe { mem::transmute(data) };

	user_data.readable.as_ref().map(|cb| cb());
}

extern "C" fn writable_callback(_:    *mut c_int,
                                data: *mut c_int)
{
	let user_data:&Callbacks = unsafe { mem::transmute(data) };

	user_data.writable.as_ref().map(|cb| cb());
}

extern "C" fn aborted_callback(_:     *mut c_int,
                              error: c_int,
                              data:  *mut c_int)
{
	debug!("aborted_callback");
	let err = if error == 0 {
		None
	} else {
		Some(io::Error::from_raw_os_error(error))
	};
	
	let user_data:&Callbacks = unsafe { mem::transmute(data) };

	user_data.aborted.as_ref().map(|cb| cb(err));
}

extern "C" fn write_packet(_:    *mut c_int,
                           buf:  *const c_int,
                           len:  c_int,
                           data: *mut c_int)
	-> c_uint
{
	unsafe {
		let user_data:*const Callbacks = mem::transmute(data);
		let buf = slice::from_raw_parts(buf as *mut c_uchar, len as usize);

		(*user_data).write_packet.as_ref()
		                      .map(|cb| cb(buf))
		                      .unwrap_or(ffi::WR_FAIL)
		                      .bits()
	}
}

impl PseudoTcpSocket {
	pub fn new(conversation: i32, user_callbacks: Callbacks) -> PseudoTcpSocket {
		let mut user_callbacks = Box::new(user_callbacks);

		// from the docs:
		// If the callbacks structure was dynamicly allocated, it can be freed
		// after the call pseudo_tcp_socket_new
		let mut callbacks = ffi::PseudoTcpCallbacks {
			user_data:         unsafe { mem::transmute(&mut *user_callbacks) },
			PseudoTcpOpened:   Some(opened_callback),
			PseudoTcpReadable: Some(readable_callback),
			PseudoTcpWritable: Some(writable_callback),
			PseudoTcpClosed:   Some(aborted_callback),
			WritePacket:       Some(write_packet),
		};

		let ptr = unsafe {
			ffi::pseudo_tcp_socket_new(conversation, &mut callbacks)
		};
		assert!(!ptr.is_null());

		PseudoTcpSocket {
			ptr:       ptr,
			callbacks: user_callbacks // keep user_callbacks alive!
		}
	}

	#[must_use]
	pub fn notify_packet(&self, buf: &[u8]) -> bool {
		let ptr = buf.as_ptr() as *const i32;
		let len = buf.len() as c_int;

		let res = unsafe {
			ffi::pseudo_tcp_socket_notify_packet(self.ptr, ptr, len)
		};

		res != ffi::FALSE
	}

	pub fn notify_mtu(&self, mtu: u16) {
		unsafe {
			ffi::pseudo_tcp_socket_notify_mtu(self.ptr, mtu as c_int)
		}
	}

	pub fn notify_clock(&self) {
		unsafe {
			ffi::pseudo_tcp_socket_notify_clock(self.ptr)
		}
	}

	pub fn get_error(&self) -> Option<io::Error> {
		let res = unsafe {
			ffi::pseudo_tcp_socket_get_error(self.ptr)
		};

		if res == 0 {
			None
		} else {
			Some(io::Error::from_raw_os_error(res))
		}
	}

	#[must_use]
	pub fn connect(&self) -> io::Result<()> {
		let res = unsafe {
			ffi::pseudo_tcp_socket_connect(self.ptr)
		};

		if res == ffi::FALSE {
			Err(self.get_error().unwrap())
		} else {
			Ok(())
		}
	}

	pub fn close(&self, force: bool) {
		let force = if force { ffi::TRUE } else { ffi::FALSE };

		unsafe {
			ffi::pseudo_tcp_socket_close(self.ptr, force)
		}
	}

	#[must_use]
	/// timeout in millisec
	pub fn get_next_clock_ms(&self) -> Option<u64> {
		let mut timeout_ms = 0 as c_ulong;

		let res = unsafe {
			ffi::pseudo_tcp_socket_get_next_clock(self.ptr, &mut timeout_ms)
		};

		if res == ffi::FALSE {
			None
		} else {
			let now = unsafe {
				ffi::g_get_monotonic_time() as u64
			};
			
			timeout_ms -= now/1000;

			Some(timeout_ms)
		}
	}

	pub fn set_debug_level(level: ffi::PseudoTcpDebugLevel) {
		unsafe {
			ffi::pseudo_tcp_set_debug_level(level.bits());
		}
	}

	/// If this function return -1 with EWOULDBLOCK as the error, or if the
	/// return value is lower than len, then the
	/// PseudoTcpCallbacks:PseudoTcpWritable callback will be called when the
	/// socket will become writable. 
	pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
		let ptr = buf.as_ptr() as *const c_char;
		let len = buf.len() as c_int;

		let res = unsafe {
			ffi::pseudo_tcp_socket_send(self.ptr, ptr, len)
		};

		if res < 0 {
			Err(self.get_error().unwrap())
		} else {
			Ok(res as usize)
		}
	}

	pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
		let ptr = buf.as_mut_ptr() as *mut c_char;
		let len = buf.len() as c_int;

		let res = unsafe {
			ffi::pseudo_tcp_socket_recv(self.ptr, ptr, len)
		};

		if res < 0 {
			let error = self.get_error().unwrap();
			debug!("pseudo_tcp_socket_recv()=={:?}", error);
			Err(error)
		}
		else {
			Ok(res as usize)
		}
	}
}

impl Drop for PseudoTcpSocket {
	fn drop(&mut self) {
		unsafe {
			ffi::g_object_unref(self.ptr);
		}
	}
}
