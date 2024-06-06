use core::{
    cell::Cell,
    task::{Context, RawWaker, RawWakerVTable, Waker},
};

#[cfg(feature = "std")]
use std::thread::Thread;

#[cfg(feature = "std")]
pub struct ThreadParkToken(Cell<Option<Thread>>);
pub struct AsyncParkToken(Cell<Option<Waker>>);
#[cfg(feature = "std")]
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
    unsafe fn wake(&self);
}

#[cfg(feature = "std")]
impl seal::Seal for ThreadParkToken {}
// # SAFETY: thread::park doesn't unwind
#[cfg(feature = "std")]
unsafe impl Parker for ThreadParkToken {
    #[doc(hidden)]
    const NEW: Self = ThreadParkToken(Cell::new(None));

    #[doc(hidden)]
    unsafe fn wake(&self) {
        if let Some(thread) = self.0.take() {
            thread.unpark()
        }
    }
}

#[cfg(feature = "std")]
impl ThreadParkToken {
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }

    pub fn set(&self) {
        self.0.set(Some(std::thread::current()))
    }

    pub fn clear(&self) {
        self.0.set(None)
    }
}

impl AsyncParkToken {
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }

    pub fn set(&self, ctx: &mut Context) {
        self.0.set(Some(ctx.waker().clone()))
    }

    pub fn clear(&self) {
        self.0.set(None);
    }
}

impl seal::Seal for AsyncParkToken {}
// SAFETY: there is a panic guard to ensure that wake doesn't unwind
unsafe impl Parker for AsyncParkToken {
    #[doc(hidden)]
    const NEW: Self = Self::new();

    #[doc(hidden)]
    unsafe fn wake(&self) {
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
impl seal::Seal for AdaptiveParkToken {}
// # SAFETY: Parker::wake can't unwind for thread_token and async_token
#[cfg(feature = "std")]
unsafe impl Parker for AdaptiveParkToken {
    #[doc(hidden)]
    const NEW: Self = AdaptiveParkToken {
        thread_token: ThreadParkToken::NEW,
        async_token: AsyncParkToken::NEW,
    };

    #[doc(hidden)]
    unsafe fn wake(&self) {
        // SAFETY: ensured by caller
        unsafe {
            self.thread_token.wake();
            self.async_token.wake();
        }
    }
}
