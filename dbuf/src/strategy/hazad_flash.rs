//! this strategy was inspired by the flashmap crate
//!
//! see [`flashmap`](https://docs.rs/flashmap/latest/flashmap/) for more details

use crate::{
    interface::{AsyncStrategy, Strategy},
    strategy::hazard::ReleaseOnDrop,
};
use core::{
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
    task::Poll,
};

use const_fn::const_fn;
use sync_wrapper::SyncWrapper;

#[cfg(feature = "std")]
use super::flash_park_token::{AdaptiveParkToken, ThreadParkToken};
use super::{
    flash_park_token::{AsyncParkToken, Parker},
    hazard::{Hazard, RawHazardGuard},
};

#[cfg(test)]
mod test;

pub struct HazardFlashStrategy<P> {
    swap_state: AtomicUsize,
    readers: Hazard<AtomicUsize, 4>,
    residual: AtomicIsize,
    parker: P,
}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}

    #[cfg(feature = "std")]
    let _ = send_sync::<HazardFlashStrategy<AdaptiveParkToken>>;
};

const NOT_SWAPPED: usize = 0;
const SWAPPED: usize = 1;
const READER_ACTIVE: usize = 2;

pub struct WriterId(());
pub struct ReaderId {
    id: SyncWrapper<Option<RawHazardGuard<AtomicUsize, 4>>>,
}

pub struct ReadGuard {
    swap_state: usize,
}

#[non_exhaustive]
pub struct Swap;

impl HazardFlashStrategy<AsyncParkToken> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new_async() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
impl HazardFlashStrategy<ThreadParkToken> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new_blocking() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
impl HazardFlashStrategy<AdaptiveParkToken> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new() -> Self {
        Self::with_parker()
    }
}

impl<P: Parker> HazardFlashStrategy<P> {
    #[const_fn(cfg(not(loom)))]
    const fn with_parker() -> Self {
        Self {
            swap_state: AtomicUsize::new(NOT_SWAPPED),
            readers: Hazard::new(),
            residual: AtomicIsize::new(0),
            parker: P::NEW,
        }
    }
}

impl Default for HazardFlashStrategy<AsyncParkToken> {
    #[inline]
    fn default() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
impl Default for HazardFlashStrategy<ThreadParkToken> {
    #[inline]
    fn default() -> Self {
        Self::with_parker()
    }
}

#[cfg(feature = "std")]
impl Default for HazardFlashStrategy<AdaptiveParkToken> {
    #[inline]
    fn default() -> Self {
        Self::with_parker()
    }
}

impl<P: Parker> HazardFlashStrategy<P> {
    fn create_reader_id(&self) -> ReaderId {
        let id = self.readers.get_or_insert_with(|| AtomicUsize::new(0));
        ReaderId {
            id: SyncWrapper::new(Some(id)),
        }
    }

    fn reader_id<'a>(&'a self, reader: &'a mut ReaderId) -> &'a AtomicUsize {
        let reader_id = (reader.id.get_mut())
            .get_or_insert_with(|| self.readers.get_or_insert_with(|| AtomicUsize::new(0)));
        // SAFETY: the hazard is still alive, since the HazardFlashStrategy contains it
        unsafe { reader_id.as_ref() }
    }
}

impl Drop for ReaderId {
    fn drop(&mut self) {
        if let Some(id) = self.id.get_mut() {
            // SAFETY: The reader is is only created in create_reader_id_from_* which require the
            // id is dropped before the strategy, so if we have reached this point then
            // the Hazard is still alive, which keeps all the nodes alive
            unsafe { id.release_if_locked() }
        }
    }
}

// SAFETY: FlashStrategy when used as a strategy for a double buffer is thread safe
// because finish_swap doesn't return while there are any readers in the
// buffer that the writer (even if the readers are on other threads). see the module
// docs for more information on the particular algorithm.
unsafe impl<P: Parker> Strategy for HazardFlashStrategy<P> {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap;
    type SwapError = core::convert::Infallible;

    type ReadGuard = ReadGuard;

    #[inline]
    unsafe fn create_writer_id(&mut self) -> Self::WriterId {
        WriterId(())
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
        let swap_state = unsafe { core::ptr::read(&self.swap_state).into_inner() };
        swap_state != NOT_SWAPPED
    }

    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool {
        guard.swap_state != NOT_SWAPPED
    }

    unsafe fn try_start_swap(
        &self,
        _writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        let old_swap_state = self.swap_state.fetch_xor(SWAPPED, Ordering::Release);

        let residual_swap_state = old_swap_state | READER_ACTIVE;
        let mut residual = 0;

        for reader in self.readers.iter() {
            let reader_swap_state = reader.fetch_xor(1, Ordering::AcqRel);

            // This increment is bounded by the number of readers there are
            // which can never exceed isize::MAX (because of the max allocation
            // size of readers) so this increment can never overflow
            #[allow(clippy::arithmetic_side_effects)]
            if reader_swap_state == residual_swap_state {
                residual += 1;
            }
        }

        self.residual.fetch_add(residual, Ordering::Release);

        Ok(Swap)
    }

    unsafe fn is_swap_finished(&self, _writer: &mut Self::WriterId, Swap: &mut Self::Swap) -> bool {
        self.residual.load(Ordering::Acquire) == 0
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let reader_id = if let Some(reader_id) = reader.id.get_mut() {
            // SAFETY: reader is associated from the this HazardFlashStrategy
            // so the RawHazardGuard is still valid
            if unsafe { reader_id.try_acquire() } {
                // SAFETY: reader is associated from the this HazardFlashStrategy
                // so the RawHazardGuard is still valid
                unsafe { reader_id.as_ref() }
            } else {
                self.reader_id(reader)
            }
        } else {
            self.reader_id(reader)
        };

        assert_eq!(
            reader_id.load(Ordering::Relaxed) & READER_ACTIVE,
            0,
            "Detected a leaked read guard"
        );

        let id = reader_id.fetch_or(READER_ACTIVE, Ordering::Release);
        ReadGuard { swap_state: id }
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, guard: Self::ReadGuard) {
        {
            let reader_guard = reader.id.get_mut().as_mut();
            // SAFETY: the reader was previously acquired, so this must be Some
            let reader_guard = unsafe { reader_guard.unwrap_unchecked() };
            let reader_guard = ReleaseOnDrop(reader_guard);
            // SAFETY: the reader was previously acquired, so it must be locked
            let reader_id = unsafe { reader_guard.0.as_ref() };
            let reader_swap_state = reader_id.fetch_and(!READER_ACTIVE, Ordering::Release);

            // if there wasn't any intervening swap then just return
            if guard.swap_state & 1 == reader_swap_state & 1 {
                return;
            }

            // if was an intervening swap, then this is a residual reader
            // from the last swap. So we should register it as such

            let residual = self.residual.fetch_sub(1, Ordering::AcqRel);

            // if there are more residual readers, then someone else will wake up the writer
            if residual != 1 {
                return;
            }
        }

        // if this is the last residual reader, then wake up the writer

        // SAFETY: residual is non-zero
        unsafe { self.parker.wake() }
    }
}

// SAFETY: we check if is_swap_finished would return true before returning Poll::Ready
unsafe impl AsyncStrategy for HazardFlashStrategy<AsyncParkToken> {
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> Poll<()> {
        self.poll(swap, |should_set| {
            if should_set {
                self.parker.set(ctx)
            } else {
                self.parker.clear();
            }
        })
    }
}

#[cfg(feature = "std")]
// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl crate::interface::BlockingStrategy for HazardFlashStrategy<ThreadParkToken> {
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, mut swap: Self::Swap) {
        if self
            .poll(&mut swap, |should_set| {
                if should_set {
                    self.parker.set()
                } else {
                    self.parker.clear();
                }
            })
            .is_pending()
        {
            while self.residual.load(Ordering::Relaxed) != 0 {
                std::thread::park();
            }
        }
    }
}

#[cfg(feature = "std")]
// SAFETY: we check if is_swap_finished would return true before returning Poll::Ready
unsafe impl AsyncStrategy for HazardFlashStrategy<AdaptiveParkToken> {
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> Poll<()> {
        self.poll(swap, |should_set| {
            if should_set {
                self.parker.async_token.set(ctx)
            } else {
                self.parker.async_token.clear();
            }
        })
    }
}

#[cfg(feature = "std")]
// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl crate::interface::BlockingStrategy for HazardFlashStrategy<AdaptiveParkToken> {
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, mut swap: Self::Swap) {
        if self
            .poll(&mut swap, |should_set| {
                if should_set {
                    self.parker.thread_token.set()
                } else {
                    self.parker.thread_token.clear();
                }
            })
            .is_pending()
        {
            while self.residual.load(Ordering::Relaxed) != 0 {
                std::thread::park();
            }
        }
    }
}

impl<T> HazardFlashStrategy<T> {
    fn poll(&self, Swap: &mut Swap, mut setup: impl FnMut(bool)) -> Poll<()> {
        if self.residual.load(Ordering::Acquire) == 0 {
            return Poll::Ready(());
        }

        setup(true);
        let residual = self.residual.load(Ordering::Acquire);
        // if all residual readers finished already
        if residual == 0 {
            setup(false);
            return Poll::Ready(());
        }

        Poll::Pending
    }
}
