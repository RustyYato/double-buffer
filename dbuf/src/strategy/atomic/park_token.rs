#[cfg(feature = "atomic-waker")]
use core::task::Context;

#[cfg(feature = "std")]
use std::sync::{Condvar, Mutex, PoisonError};

#[cfg(feature = "atomic-waker")]
use atomic_waker::AtomicWaker;

#[cfg(feature = "std")]
#[derive(Default)]
pub struct ThreadParkToken {
    mutex: Mutex<()>,
    cv: Condvar,
}
#[cfg(feature = "atomic-waker")]
#[derive(Default)]
pub struct AsyncParkToken(AtomicWaker);
#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
#[derive(Default)]
pub struct AdaptiveParkToken {
    pub(crate) thread_token: ThreadParkToken,
    pub(crate) async_token: AsyncParkToken,
}

mod seal {
    pub trait Seal {}
}

/// This is an internal trait, do not use it directly.
///
/// It only exists as an implementation detail to abstract over [`ThreadParkToken`],
/// [`AsyncParkToken`], and [`AdaptiveParkToken`]
///
/// This trait is sealed, so you cannot implement this trait.
///
/// # Safety
///
/// ParkToken::wake must not unwind
pub unsafe trait Parker: Sized + seal::Seal {
    #[doc(hidden)]
    const NEW: Self;

    #[doc(hidden)]
    #[allow(unused)]
    fn wake(&self);
}

impl seal::Seal for () {}
// SAFETY: Parker::wake is a nop
unsafe impl Parker for () {
    const NEW: Self = ();

    fn wake(&self) {}
}

#[cfg(feature = "std")]
impl seal::Seal for ThreadParkToken {}
#[cfg(feature = "std")]
// SAFETY: thread::park doesn't unwind
unsafe impl Parker for ThreadParkToken {
    #[doc(hidden)]
    const NEW: Self = ThreadParkToken {
        mutex: Mutex::new(()),
        cv: Condvar::new(),
    };

    #[doc(hidden)]
    fn wake(&self) {
        self.cv.notify_one();
    }
}

#[cfg(feature = "std")]
impl ThreadParkToken {
    pub(in crate::strategy) fn park_until(&self, mut f: impl FnMut() -> bool) {
        let mut guard = self.mutex.lock().unwrap_or_else(PoisonError::into_inner);
        while !f() {
            guard = self.cv.wait(guard).unwrap_or_else(PoisonError::into_inner)
        }
    }
}

#[cfg(feature = "atomic-waker")]
impl AsyncParkToken {
    pub const fn new() -> Self {
        Self(AtomicWaker::new())
    }

    pub(in crate::strategy) fn set(&self, ctx: &mut Context) {
        self.0.register(ctx.waker())
    }

    pub(in crate::strategy) fn clear(&self) {
        self.0.take();
    }
}

#[cfg(feature = "atomic-waker")]
impl seal::Seal for AsyncParkToken {}
#[cfg(feature = "atomic-waker")]
// SAFETY: there is a panic guard to ensure that wake doesn't unwind
unsafe impl Parker for AsyncParkToken {
    #[doc(hidden)]
    const NEW: Self = Self::new();

    #[doc(hidden)]
    fn wake(&self) {
        struct Bomb;

        impl Drop for Bomb {
            fn drop(&mut self) {
                panic!("Tried to panic out of an async Waker::wake")
            }
        }

        let guard = Bomb;
        if let Some(waker) = self.0.take() {
            waker.wake()
        }
        core::mem::forget(guard);
    }
}

#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
impl seal::Seal for AdaptiveParkToken {}
#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
// SAFETY: Parker::wake can't unwind for thread_token and async_token
unsafe impl Parker for AdaptiveParkToken {
    #[doc(hidden)]
    const NEW: Self = AdaptiveParkToken {
        thread_token: ThreadParkToken::NEW,
        async_token: AsyncParkToken::NEW,
    };

    #[doc(hidden)]
    fn wake(&self) {
        self.thread_token.wake();
        self.async_token.wake();
    }
}
