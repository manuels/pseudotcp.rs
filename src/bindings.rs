#![allow(non_snake_case)]
#![allow(dead_code)]
extern crate libc;

pub const FALSE: libc::c_int = 0;
pub const TRUE:  libc::c_int = !FALSE;

pub const EAGAIN:  libc::c_int = 11;

/*
struct _PseudoTcpSocketClass
		(int) parent_class
*/
#[repr(C)]
pub struct _PseudoTcpSocketClass {
	parent_class: libc::c_int,
}

/*
struct _PseudoTcpSocketPrivate
*/
#[repr(C)]
pub struct _PseudoTcpSocketPrivate;

/*
struct _PseudoTcpSocket
		(int) parent
		(PseudoTcpSocketPrivate *) priv [struct _PseudoTcpSocketPrivate *]
*/
#[repr(C)]
pub struct _PseudoTcpSocket {
	parent: libc::c_int,
	priv_: *mut _PseudoTcpSocketPrivate,
}

/*
struct 
		(int) user_data
		(void (*)(int *, int)) PseudoTcpOpened [void (*)(int *, int)]
		(void (*)(int *, int)) PseudoTcpReadable [void (*)(int *, int)]
		(void (*)(int *, int)) PseudoTcpWritable [void (*)(int *, int)]
		(void (*)(int *, int, int)) PseudoTcpClosed [void (*)(int *, int, int)]
		(PseudoTcpWriteResult (*)(int *, const int *, int, int)) WritePacket [PseudoTcpWriteResult (*)(int *, const int *, int, int)]
#[repr(C)]
pub struct  {
	user_data: libc::c_int,
	PseudoTcpOpened: Option<extern fn(*mut libc::c_int, libc::c_int)>,
	PseudoTcpReadable: Option<extern fn(*mut libc::c_int, libc::c_int)>,
	PseudoTcpWritable: Option<extern fn(*mut libc::c_int, libc::c_int)>,
	PseudoTcpClosed: Option<extern fn(*mut libc::c_int, libc::c_int, libc::c_int)>,
	WritePacket: Option<extern fn(*mut libc::c_int, *const libc::c_int, libc::c_int, libc::c_int) -> libc::c_uint>,
}
*/

/*
struct PseudoTcpCallbacks
		(int) user_data
		(void (*)(int *, int)) PseudoTcpOpened [void (*)(int *, int)]
		(void (*)(int *, int)) PseudoTcpReadable [void (*)(int *, int)]
		(void (*)(int *, int)) PseudoTcpWritable [void (*)(int *, int)]
		(void (*)(int *, int, int)) PseudoTcpClosed [void (*)(int *, int, int)]
		(PseudoTcpWriteResult (*)(int *, const int *, int, int)) WritePacket [PseudoTcpWriteResult (*)(int *, const int *, int, int)]
*/
#[repr(C)]
pub struct PseudoTcpCallbacks {
	pub user_data: *mut libc::c_int,
	pub PseudoTcpOpened: Option<extern fn(*mut libc::c_int, *mut libc::c_int)>,
	pub PseudoTcpReadable: Option<extern fn(*mut libc::c_int, *mut libc::c_int)>,
	pub PseudoTcpWritable: Option<extern fn(*mut libc::c_int, *mut libc::c_int)>,
	pub PseudoTcpClosed: Option<extern fn(*mut libc::c_int, libc::c_int, *mut libc::c_int)>,
	pub WritePacket: Option<extern fn(*mut libc::c_int, *const libc::c_int, libc::c_int, *mut libc::c_int) -> libc::c_uint>,
}

/*
int pseudo_tcp_socket_get_type()
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_get_type() -> libc::c_int;
}

#[link(name="glib-2.0")]
extern "C" {
	pub fn g_type_init();
}

#[link(name="glib-2.0")]
extern "C" {
	pub fn g_object_ref(ptr: *mut _PseudoTcpSocket);
	pub fn g_object_unref(ptr: *mut _PseudoTcpSocket);
}

/*
int * pseudo_tcp_socket_new()
	(int) conversation
	(PseudoTcpCallbacks *) callbacks [PseudoTcpCallbacks *]
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_new(conversation: libc::c_int, callbacks: *mut PseudoTcpCallbacks) -> *mut _PseudoTcpSocket;
}


/*
int pseudo_tcp_socket_connect()
	(int *) self
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_connect(self_: *mut _PseudoTcpSocket) -> libc::c_int;
}


/*
int pseudo_tcp_socket_recv()
	(int *) self
	(char *) buffer
	(int) len
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_recv(self_: *mut _PseudoTcpSocket, buffer: *mut libc::c_char, len: libc::c_int) -> libc::c_int;
}


/*
int pseudo_tcp_socket_send()
	(int *) self
	(const char *) buffer
	(int) len
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_send(self_: *mut _PseudoTcpSocket, buffer: *const libc::c_char, len: libc::c_int) -> libc::c_int;
}


/*
void pseudo_tcp_socket_close()
	(int *) self
	(int) force
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_close(self_: *mut _PseudoTcpSocket, force: libc::c_int);
}


/*
int pseudo_tcp_socket_get_error()
	(int *) self
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_get_error(self_: *mut _PseudoTcpSocket) -> libc::c_int;
}


/*
int pseudo_tcp_socket_get_next_clock()
	(int *) self
	(long *) timeout
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_get_next_clock(self_: *mut _PseudoTcpSocket, timeout: *mut libc::c_long) -> libc::c_int;
}


/*
void pseudo_tcp_socket_notify_clock()
	(int *) self
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_notify_clock(self_: *mut _PseudoTcpSocket);
}


/*
void pseudo_tcp_socket_notify_mtu()
	(int *) self
	(int) mtu
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_notify_mtu(self_: *mut _PseudoTcpSocket, mtu: libc::c_int);
}


/*
int pseudo_tcp_socket_notify_packet()
	(int *) self
	(const int *) buffer
	(int) len
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_socket_notify_packet(self_: *mut _PseudoTcpSocket, buffer: *const libc::c_int, len: libc::c_int) -> libc::c_int;
}


/*
void pseudo_tcp_set_debug_level()
	(PseudoTcpDebugLevel) level [PseudoTcpDebugLevel]
*/
#[link(name="nice")]
extern "C" {
	pub fn pseudo_tcp_set_debug_level(level: libc::c_uint);
}


/*
enum  {
	PSEUDO_TCP_DEBUG_NONE =	0x00000000 (0)
	PSEUDO_TCP_DEBUG_NORMAL =	0x00000001 (1)
	PSEUDO_TCP_DEBUG_VERBOSE =	0x00000002 (2)
}
*/
bitflags! {
	flags PseudoTcpDebugLevel: libc::c_uint {
		const PSEUDO_TCP_DEBUG_NONE =	0 as libc::c_uint,
		const PSEUDO_TCP_DEBUG_NORMAL =	1 as libc::c_uint,
		const PSEUDO_TCP_DEBUG_VERBOSE =	2 as libc::c_uint,
	}
}


/*
enum  {
	TCP_LISTEN =	0x00000000 (0)
	TCP_SYN_SENT =	0x00000001 (1)
	TCP_SYN_RECEIVED =	0x00000002 (2)
	TCP_ESTABLISHED =	0x00000003 (3)
	TCP_CLOSED =	0x00000004 (4)
}
*/
bitflags! {
	flags PseudoTcpState: libc::c_uint {
		const TCP_LISTEN =	0 as libc::c_uint,
		const TCP_SYN_SENT =	1 as libc::c_uint,
		const TCP_SYN_RECEIVED =	2 as libc::c_uint,
		const TCP_ESTABLISHED =	3 as libc::c_uint,
		const TCP_CLOSED =	4 as libc::c_uint,
	}
}

/*
enum  {
	WR_SUCCESS =	0x00000000 (0)
	WR_TOO_LARGE =	0x00000001 (1)
	WR_FAIL =	0x00000002 (2)
}
*/
bitflags! {
	flags PseudoTcpWriteResult: libc::c_uint {
		const WR_SUCCESS =	0 as libc::c_uint,
		const WR_TOO_LARGE =	1 as libc::c_uint,
		const WR_FAIL =	2 as libc::c_uint,
	}
}



/* _PSEUDOTCP_H /**
 * SECTION:pseudotcp
 * @short_description: Pseudo TCP implementation
 * @include: pseudotcp.h
 * @stability: Stable
 *
 * The #PseudoTcpSocket is an object implementing a Pseudo Tcp Socket for use
 * over UDP.
 * The socket will implement a subset of the TCP stack to allow for a reliable
 * transport over non-reliable sockets (such as UDP).
 *
 * See the file tests/test-pseudotcp.c in the source package for an example
 * of how to use the object.
 *
 * Since: 0.0.11
 */ */

/* PSEUDO_TCP_SOCKET_TYPE ( pseudo_tcp_socket_get_type ( ) ) # */

/* PSEUDO_TCP_SOCKET ( obj ) ( G_TYPE_CHECK_INSTANCE_CAST ( ( obj ) , PSEUDO_TCP_SOCKET_TYPE , PseudoTcpSocket ) ) # */

/* PSEUDO_TCP_SOCKET_CLASS ( klass ) ( G_TYPE_CHECK_CLASS_CAST ( ( klass ) , PSEUDO_TCP_SOCKET_TYPE , PseudoTcpSocketClass ) ) # */

/* IS_PSEUDO_TCP_SOCKET ( obj ) ( G_TYPE_CHECK_INSTANCE_TYPE ( ( obj ) , PSEUDO_TCP_SOCKET_TYPE ) ) # */

/* IS_PSEUDO_TCP_SOCKET_CLASS ( klass ) ( G_TYPE_CHECK_CLASS_TYPE ( ( klass ) , PSEUDO_TCP_SOCKET_TYPE ) ) # */

/* PSEUDOTCP_SOCKET_GET_CLASS ( obj ) ( G_TYPE_INSTANCE_GET_CLASS ( ( obj ) , PSEUDO_TCP_SOCKET_TYPE , PseudoTcpSocketClass ) ) struct */

