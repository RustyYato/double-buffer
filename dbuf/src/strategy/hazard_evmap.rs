//! this strategy was inspired by the evmap crate
//!
//! see [`evmap`](https://docs.rs/evmap/latest/evmap/) for more details

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::{interface::Strategy, strategy::hazard::ReleaseOnDrop};

#[cfg(any(feature = "std", feature = "atomic-waker"))]
use const_fn::const_fn;
use sync_wrapper::SyncWrapper;

#[cfg(any(feature = "std", feature = "atomic-waker"))]
use super::atomic::park_token;

use super::{
    atomic::park_token::Parker,
    hazard::{Hazard, RawHazardGuard, RawHazardIter},
};

#[cfg(test)]
mod test;

pub struct HazardEvMapStrategy<P: Parker> {
    is_swapped: AtomicBool,
    epochs: Hazard<Epoch, 4>,
    parker: P,
}

struct Epoch {
    current: AtomicUsize,
    last: UnsafeCell<usize>,
}

impl Epoch {
    const fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            last: UnsafeCell::new(0),
        }
    }
}

// SAFETY: Epoch is morally a `(current: AtomicUsize, last: usize)` but since formally `last` is shared
// we need to use an `UnsafeCell`. However it is only accessed by the writer, and there is only
// one writer at any given time, so this is safe.
unsafe impl Send for Epoch {}
// SAFETY: Epoch is morally a `(current: AtomicUsize, last: usize)` but since formally `last` is shared
// we need to use an `UnsafeCell`. However, `last` is never accessed by multiple threads at the same time
unsafe impl Sync for Epoch {}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}

    #[cfg(feature = "std")]
    let _ = send_sync::<HazardEvMapStrategy<park_token::ThreadParkToken>>;
    #[cfg(feature = "atomic-waker")]
    let _ = send_sync::<HazardEvMapStrategy<park_token::AsyncParkToken>>;
    #[cfg(feature = "std")]
    #[cfg(feature = "atomic-waker")]
    let _ = send_sync::<HazardEvMapStrategy<park_token::AdaptiveParkToken>>;
};

#[non_exhaustive]
pub struct WriterId;
pub struct ReaderId {
    id: SyncWrapper<Option<RawHazardGuard<Epoch, 4>>>,
}

#[cfg(feature = "atomic-waker")]
impl HazardEvMapStrategy<park_token::AsyncParkToken> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new_async() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
impl HazardEvMapStrategy<park_token::ThreadParkToken> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new_blocking() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
impl HazardEvMapStrategy<park_token::AdaptiveParkToken> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new() -> Self {
        Self::with_parker()
    }
}

impl<P: Parker> HazardEvMapStrategy<P> {
    #[const_fn(cfg(not(loom)))]
    #[cfg(any(feature = "std", feature = "atomic-waker"))]
    const fn with_parker() -> Self {
        Self {
            is_swapped: AtomicBool::new(false),
            epochs: Hazard::new(),
            parker: P::NEW,
        }
    }
}

#[cfg(feature = "atomic-waker")]
impl Default for HazardEvMapStrategy<park_token::AsyncParkToken> {
    #[inline]
    fn default() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
impl Default for HazardEvMapStrategy<park_token::ThreadParkToken> {
    #[inline]
    fn default() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
impl Default for HazardEvMapStrategy<park_token::AdaptiveParkToken> {
    #[inline]
    fn default() -> Self {
        Self::with_parker()
    }
}

impl<P: Parker> HazardEvMapStrategy<P> {
    fn create_reader_id(&self) -> ReaderId {
        let id = self.epochs.get_or_insert_with(Epoch::new);
        ReaderId {
            id: SyncWrapper::new(Some(id)),
        }
    }

    fn reader_id<'a>(&'a self, reader: &'a mut ReaderId) -> &'a AtomicUsize {
        let reader_id =
            (reader.id.get_mut()).get_or_insert_with(|| self.epochs.get_or_insert_with(Epoch::new));
        // SAFETY: the hazard is still alive, since the HazardEvMapStrategy contains it
        &unsafe { reader_id.as_ref() }.current
    }
}

pub struct Swap {
    epochs: RawHazardIter<Epoch, 4>,
}

// SAFETY: FlashStrategy when used as a strategy for a double buffer is thread safe
// because finish_swap doesn't return while there are any readers in the
// buffer that the writer (even if the readers are on other threads). see the module
// docs for more information on the particular algorithm.
unsafe impl<P: Parker> Strategy for HazardEvMapStrategy<P> {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap;
    type SwapError = core::convert::Infallible;

    type ReadGuard = bool;

    #[inline]
    unsafe fn create_writer_id(&mut self) -> Self::WriterId {
        WriterId
    }

    #[inline]
    unsafe fn create_reader_id_from_writer(&self, _writer: &Self::WriterId) -> Self::ReaderId {
        self.create_reader_id()
    }

    #[inline]
    unsafe fn create_reader_id_from_reader(&self, _reader: &Self::ReaderId) -> Self::ReaderId {
        self.create_reader_id()
    }

    #[cold]
    #[inline(never)]
    fn create_invalid_reader_id() -> Self::ReaderId {
        ReaderId {
            id: SyncWrapper::new(None),
        }
    }

    unsafe fn is_swapped_writer(&self, _writer: &Self::WriterId) -> bool {
        // SAFETY: The only write to self.swap_state happens in try_start_swap
        // which needs a &mut Self::WriterId, but we current hold a &Self::WriterId.
        //
        // There are at most 1 Self::WriterId's associated with a given strategy at a time.
        //
        // So there must be some synchronization between this and `try_start_swap`.
        // So there can be no race between that write and this read.
        //
        // And it is fine to race two (non-atomic) reads
        unsafe { core::ptr::read(&self.is_swapped).into_inner() }
    }

    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool {
        *guard
    }

    unsafe fn try_start_swap(
        &self,
        _writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        self.is_swapped.fetch_xor(true, Ordering::AcqRel);

        for epoch in self.epochs.iter() {
            // This needs to syncronize with [acquire|release]_read_guard (so needs `Acquire`)
            let current = epoch.current.load(Ordering::Acquire);
            // SAFETY: the reader doesn't touch epoch.last, and there is only a single valid writer id
            // associated with this strategy, which we have a &mut reference to, so there is no
            // way this write races with anything
            unsafe { epoch.last.get().write(current) }
        }

        Ok(Swap {
            // SAFETY: the caller ensures that the swap is dropped before this strategy
            // so it won't outlive the Hazard stored in the starategy
            epochs: unsafe { self.epochs.raw_iter() },
        })
    }

    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        // SAFETY: the caller ensures that `swap` is the latest swap and that it's still valid
        unsafe { is_swap_finished(writer, swap) }
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let reader_id = if let Some(reader_id) = reader.id.get_mut() {
            // SAFETY: reader is associated from the this HazardEvMapStrategy
            // so the RawHazardGuard is still valid
            if unsafe { reader_id.try_acquire() } {
                // SAFETY: reader is associated from the this HazardEvMapStrategy
                // so the RawHazardGuard is still valid
                &unsafe { reader_id.as_ref() }.current
            } else {
                self.reader_id(reader)
            }
        } else {
            self.reader_id(reader)
        };

        // this needs to syncronize with `try_start_swap`/`is_swap_finished` (so needs at least `Release`) and
        // it needs to prevent reads from the `raw::ReaderGuard` from being reordered before this (so needs at least `Acquire`)
        // the cheapest ordering which satisfies this is `AcqRel`
        reader_id.fetch_add(1, Ordering::AcqRel);
        self.is_swapped.load(Ordering::Acquire)
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, _guard: Self::ReadGuard) {
        {
            let reader_guard = reader.id.get_mut().as_mut();
            // SAFETY: the reader was previously acquired, so this must be Some
            let reader_guard = unsafe { reader_guard.unwrap_unchecked() };
            let reader_guard = ReleaseOnDrop(reader_guard);

            // SAFETY: this guard comes from the Hazard in the strategy, which is still alive
            let reader_id = &unsafe { reader_guard.0.as_ref() }.current;

            // this needs to syncronize with `try_start_swap`/`is_swap_finished` (so needs at least `Release`) and
            // it needs to prevent reads from the `raw::ReaderGuard` from being reordered after this (so needs at least `Release`)
            // the cheapest ordering which satisfies this is `Release`
            reader_id.fetch_add(1, Ordering::Release);
        }

        self.parker.wake()
    }
}

#[cfg(feature = "atomic-waker")]
// SAFETY: is_swap_finished always returns true
unsafe impl crate::interface::AsyncStrategy for HazardEvMapStrategy<park_token::AsyncParkToken> {
    #[inline]
    unsafe fn register_context(
        &self,
        writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        // SAFETY: the caller ensures that writer and swap are valid
        if unsafe { self.is_swap_finished(writer, swap) } {
            self.parker.clear();
            core::task::Poll::Ready(())
        } else {
            self.parker.set(ctx);
            core::task::Poll::Pending
        }
    }
}

#[cfg(feature = "std")]
// SAFETY: is_swap_finished always returns true
unsafe impl crate::interface::BlockingStrategy
    for HazardEvMapStrategy<park_token::ThreadParkToken>
{
    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, mut swap: Self::Swap) {
        self.parker
            // SAFETY: the caller ensures that writer and swap are valid
            .park_until(|| unsafe { is_swap_finished(writer, &mut swap) });
    }
}

#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
// SAFETY: is_swap_finished always returns true
unsafe impl crate::interface::AsyncStrategy for HazardEvMapStrategy<park_token::AdaptiveParkToken> {
    #[inline]
    unsafe fn register_context(
        &self,
        writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        // SAFETY: the caller ensures that writer and swap are valid
        if unsafe { self.is_swap_finished(writer, swap) } {
            self.parker.async_token.clear();
            core::task::Poll::Ready(())
        } else {
            self.parker.async_token.set(ctx);
            core::task::Poll::Pending
        }
    }
}

#[cfg(feature = "std")]
#[cfg(feature = "atomic-waker")]
// SAFETY: is_swap_finished always returns true
unsafe impl crate::interface::BlockingStrategy
    for HazardEvMapStrategy<park_token::AdaptiveParkToken>
{
    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, mut swap: Self::Swap) {
        self.parker.async_token.clear();
        self.parker
            .thread_token
            // SAFETY: the caller ensures that writer and swap are valid
            .park_until(|| unsafe { is_swap_finished(writer, &mut swap) });
    }
}

unsafe fn is_swap_finished(_writer: &mut WriterId, swap: &mut Swap) -> bool {
    loop {
        let epochs = swap.epochs.clone();

        let Some(epoch) = swap.epochs.next() else {
            break;
        };

        // SAFETY: the caller ensures that the swap is valid, so it always yields valid epochs
        let epoch = unsafe { epoch.as_ref() };
        // SAFETY: we have access to &mut WriterId, which is derived from some methods on
        // `Strategy`, all which require their callers ensure that there pass a valid
        // &mut WriterId. And there is at most 1 valid WriterId at any given time. So this
        // read cannot race with anything.
        let last_epoch = unsafe { epoch.last.get().read() };
        let epoch = &epoch.current;

        // if the reader wasn't reading at the start of the swap, then it cannot be in the current buffer
        if last_epoch % 2 == 0 {
            continue;
        }

        // This needs to syncronize with [acquire|release]_read_guard (so needs `Acquire`)
        let now = epoch.load(Ordering::Acquire);

        // swap.range.start < epochs.len() - i,  so
        // swap.range.start + i < epochs.len(),  so
        // `swap.range.start + i` cannot overflow
        #[allow(clippy::arithmetic_side_effects)]
        if now == last_epoch {
            swap.epochs = epochs;
            return false;
        }
    }

    true
}
