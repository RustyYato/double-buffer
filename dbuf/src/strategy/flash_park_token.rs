use core::{
    cell::Cell,
    task::{Context, Waker},
};

#[cfg(feature = "std")]
use std::thread::Thread;

#[cfg(feature = "std")]
#[derive(Default)]
pub struct ThreadParkToken(Cell<Option<Thread>>);
#[derive(Default)]
pub struct AsyncParkToken(Cell<Option<Waker>>);
#[cfg(feature = "std")]
#[derive(Default)]
pub struct AdaptiveParkToken {
    pub(crate) thread_token: ThreadParkToken,
    pub(crate) async_token: AsyncParkToken,
}

#[cfg(feature = "std")]
// SAFETY: FlashStrategy ensures that all access to the park token
// by the writer only happens when the residual is negative
// and by readers when the residual is zero (and by only one reader)
//
// These two states are mutually disjoint, so they cannot race
// All other parts of the FlashStrategy are trivially thread-safe
unsafe impl Sync for ThreadParkToken {}

// SAFETY: FlashStrategy ensures that all access to the park token
// by the writer only happens when the residual is negative
// and by readers when the residual is zero (and by only one reader)
//
// These two states are mutually disjoint, so they cannot race
// All other parts of the FlashStrategy are trivially thread-safe
unsafe impl Sync for AsyncParkToken {}

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
#[cfg(feature = "std")]
// SAFETY: thread::park doesn't unwind
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

    pub(in crate::strategy) fn set(&self) {
        self.0.set(Some(std::thread::current()))
    }

    pub(in crate::strategy) fn clear(&self) {
        self.0.set(None)
    }
}

impl AsyncParkToken {
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }

    #[cfg(feature = "alloc")]
    pub(in crate::strategy) fn set(&self, ctx: &mut Context) {
        self.0.set(Some(ctx.waker().clone()))
    }

    #[cfg(feature = "alloc")]
    pub(in crate::strategy) fn clear(&self) {
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
#[cfg(feature = "std")]
// SAFETY: Parker::wake can't unwind for thread_token and async_token
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
