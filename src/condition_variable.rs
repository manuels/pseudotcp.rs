use std::cmp::PartialEq;
use std::sync::{Mutex, Condvar, PoisonError, MutexGuard, LockResult};

pub enum Notify {
	One,
	All,
}

pub struct ConditionVariable<T> {
	pair: (Mutex<T>, Condvar)
}

impl<T:PartialEq+Clone> ConditionVariable<T> {
	pub fn new(value: T) -> ConditionVariable<T> {
		ConditionVariable {
			pair: (Mutex::new(value), Condvar::new())
		}
	}

	pub fn set(&self, value: T, notify: Notify) {
		let &(ref lock, ref cvar) = &self.pair;

		let mut data = lock.lock().unwrap();
		*data = value;

		match notify {
			Notify::One => cvar.notify_one(),
			Notify::All => cvar.notify_all(),
		}
	}

	pub fn get(&self) -> Result<T, PoisonError<MutexGuard<T>>> {
		let &(ref lock, _) = &self.pair;

		let data = try!(lock.lock());

		Ok(data.clone())
	}

	pub fn wait_for(&self, expected: T) -> Result<(), PoisonError<MutexGuard<T>>> {
		let &(ref lock, ref cvar) = &self.pair;
		let mut actual = try!(lock.lock());
		
		while *actual != expected {
			actual = try!(cvar.wait(actual));
		}

		Ok(())
	}
}

impl ConditionVariable<()> {
	/// waits for a notify (useful if T==())
	pub fn wait_ms(&self, timeout_ms: u32) -> LockResult<(MutexGuard<()>,bool)>
	{
		let &(ref lock, ref cvar) = &self.pair;
		let guard = lock.lock().unwrap();
		
		cvar.wait_timeout_ms(guard, timeout_ms)
	}
}
