use core::{cell::Cell, task::Waker};

use core::task::Context;

#[cfg(feature = "std")]
use std::thread::Thread;

#[cfg(feature = "std")]
#[derive(Default)]
pub struct ThreadParkToken(Cell<Option<Thread>>);
#[derive(Default)]
pub struct AsyncParkToken(Cell<Option<Waker>>);
#[derive(Default)]
pub struct AdaptiveParkToken {
    #[cfg(feature = "std")]
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

pub trait ContextKind {
    type Context<'a, 'b>
    where
        'a: 'b;
}

pub struct ContextFamily;

impl ContextKind for () {
    type Context<'a, 'b>
        = ()
    where
        'a: 'b;
}
impl ContextKind for ContextFamily {
    type Context<'a, 'b>
        = &'b mut Context<'a>
    where
        'a: 'b;
}

pub(in crate::strategy) type Ctx<'a, 'b, C> = <C as ContextKind>::Context<'a, 'b>;

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
    type ContextKind: ContextKind;

    #[doc(hidden)]
    unsafe fn wake(&self);

    #[doc(hidden)]
    fn set(&self, ctx: Ctx<Self::ContextKind>);

    #[doc(hidden)]
    fn clear(&self);
}

#[cfg(feature = "std")]
impl seal::Seal for ThreadParkToken {}
#[cfg(feature = "std")]
// SAFETY: thread::park doesn't unwind
unsafe impl Parker for ThreadParkToken {
    #[doc(hidden)]
    const NEW: Self = ThreadParkToken(Cell::new(None));
    type ContextKind = ();

    #[doc(hidden)]
    unsafe fn wake(&self) {
        if let Some(thread) = self.0.take() {
            thread.unpark()
        }
    }

    #[inline]
    #[doc(hidden)]
    fn set(&self, _ctx: ()) {
        self.0.set(Some(std::thread::current()))
    }

    #[inline]
    #[doc(hidden)]
    fn clear(&self) {
        self.0.set(None)
    }
}

#[cfg(feature = "std")]
impl ThreadParkToken {
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }
}

impl AsyncParkToken {
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }
}
impl seal::Seal for () {}
// SAFETY: Parker::wake is a nop
unsafe impl Parker for () {
    const NEW: Self = ();
    type ContextKind = ();

    unsafe fn wake(&self) {}
    fn set(&self, (): ()) {}
    fn clear(&self) {}
}

impl seal::Seal for AsyncParkToken {}
// SAFETY: there is a panic guard to ensure that wake doesn't unwind
unsafe impl Parker for AsyncParkToken {
    #[doc(hidden)]
    const NEW: Self = Self::new();
    type ContextKind = ContextFamily;

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

    #[inline]
    #[doc(hidden)]
    fn set(&self, ctx: &mut core::task::Context<'_>) {
        self.0.set(Some(ctx.waker().clone()))
    }

    #[inline]
    #[doc(hidden)]
    fn clear(&self) {
        self.0.set(None)
    }
}

impl seal::Seal for AdaptiveParkToken {}
// SAFETY: Parker::wake can't unwind for thread_token and async_token
unsafe impl Parker for AdaptiveParkToken {
    #[doc(hidden)]
    const NEW: Self = AdaptiveParkToken {
        #[cfg(feature = "std")]
        thread_token: ThreadParkToken::NEW,
        async_token: AsyncParkToken::NEW,
    };
    type ContextKind = ContextFamily;

    #[doc(hidden)]
    unsafe fn wake(&self) {
        // SAFETY: ensured by caller
        unsafe {
            #[cfg(feature = "std")]
            self.thread_token.wake();
            self.async_token.wake();
        }
    }

    #[inline]
    #[doc(hidden)]
    fn set(&self, ctx: &mut core::task::Context<'_>) {
        #[cfg(feature = "std")]
        self.thread_token.set(());
        self.async_token.set(ctx);
    }

    #[inline]
    #[doc(hidden)]
    fn clear(&self) {
        #[cfg(feature = "std")]
        self.thread_token.clear();
        self.async_token.clear();
    }
}
