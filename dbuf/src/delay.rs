//! A delay writer is a fundamental part building block for batched writes.
//!
//! The main idea is to finish swaps right before you write a batch, and
//! then start a new swap. If you use a compatible strategy (such as
//! [`FlashStrategy`](crate::strategy::flashmap::FlashStrategy)), then this
//! enable writes to be wait-free for the most part, since all readers
//! will have finished their reads before the next batch is written.
//!
//! # Worked Example
//!
//! Here is the worked example from the crate level docs adapted for [`DelayWriter`]
//!  
//! ```rust
//! use dbuf::raw::{Writer, Reader, DoubleBufferData};
//! use dbuf::strategy::flashmap::FlashStrategy;
//!
//! use rc_box::ArcBox;
//! use std::sync::Arc;
//!
//! let front = 10;
//! let back = 300;
//!
//! let mut data = DoubleBufferData::new(front, back, FlashStrategy::new_blocking());
//! let writer: Writer<Arc<DoubleBufferData<i32, FlashStrategy<_>>>> = Writer::new(ArcBox::new(data));
//!            // ^^^ note how this is a arc now, this is for a similar reason above
//!            // use use `ArcBox`'s uniqueness guarantee to ensure
//!            // the writer has exclusive access to the buffers, but
//!            // after that, it needs to be downgraded so the buffers can be
//!            // shared between the writer and readers
//!
//! /// Convert the write to a delay writer
//! let mut writer = dbuf::delay::DelayWriter::from(writer);
//!
//! /// DelayWriter derefs to a normal writer, so you can all those methods as well
//! let mut reader = writer.reader();
//!
//! // NOTE: you should handle the errors properly, instead of using unwrap
//! // for this example, we know that the writer is still alive, so the unwrap
//! // is justified
//! let mut guard = reader.try_read().unwrap();
//!
//! // the front buffer is shown first
//! assert_eq!(*guard, 10);
//! assert_eq!(*writer.get(), 300);
//!
//! // this starts a swap, and blocks mutable access to the buffer
//! writer.start_swap();
//! // this is an error
//! // assert_eq!(*writer.get_mut(), 10);
//! assert_eq!(*writer.get(), 10);
//!
//! // end the active read, normally in your code this will happen
//! // at the end of the scope automatically.
//! drop(guard);
//!
//! // since the reader is dropped above, it is now safe to swap the buffers
//! // and finish_swap returns a mutable reference to the underlying writer
//! // so you can do whatever you want to it.
//! let writer = writer.finish_swap();
//! assert_eq!(*writer.get_mut(), 10);
//! ```

use core::fmt::Debug;
use core::ops;

use crate::{
    interface::{AsyncStrategy, BlockingStrategy, DoubleBufferWriterPointer, Strategy, SwapError},
    raw,
};

/// A batched-writer primitive
///
/// see module docs for details
pub struct DelayWriter<
    P: DoubleBufferWriterPointer,
    S: Strategy = <P as DoubleBufferWriterPointer>::Strategy,
> {
    writer: raw::Writer<P, S>,
    swap: Option<S::Swap>,
}

impl<P: DoubleBufferWriterPointer> From<raw::Writer<P>> for DelayWriter<P> {
    #[inline]
    fn from(value: raw::Writer<P>) -> Self {
        Self::from_writer(value)
    }
}

impl<P: DoubleBufferWriterPointer> DelayWriter<P> {
    /// Construct a new delay writer
    pub const fn from_writer(writer: raw::Writer<P>) -> Self {
        Self { writer, swap: None }
    }

    /// Try to start a new swap
    ///
    /// If there is already an ongoing swap, this is a no-op
    ///
    /// If there the strategy fails to swap, an error is returned
    pub fn try_start_swap(&mut self) -> Result<(), SwapError<P::Strategy>> {
        if self.swap.is_none() {
            // SAFETY: `DelayWriter` ensures that `finish_swap` or `afinish_swap`
            // is called before allowing mutable access to the `writer`
            self.swap = Some(unsafe { self.writer.try_start_swap()? })
        }

        Ok(())
    }

    /// Start a swap
    ///
    /// If there is already an ongoing swap, this is a no-op
    ///
    /// If there the strategy fails to swap, then this function panics
    pub fn start_swap(&mut self)
    where
        SwapError<P::Strategy>: Debug,
    {
        self.try_start_swap().expect("start swap must not fail")
    }

    /// Finish an ongoing swap, and return a reference to the underlying writer
    ///
    /// If there is no ongoing swap, then this is a no-op
    pub fn finish_swap(&mut self) -> &mut raw::Writer<P>
    where
        P::Strategy: BlockingStrategy,
    {
        if let Some(swap) = self.swap.take() {
            // SAFETY: this swap is the latest swap
            unsafe { self.writer.finish_swap(swap) };
        }

        &mut self.writer
    }

    /// Finish an ongoing swap, and return a reference to the underlying writer
    ///
    /// If there is no ongoing swap, then this is a no-op
    pub async fn afinish_swap(&mut self) -> &mut raw::Writer<P>
    where
        P::Strategy: AsyncStrategy,
    {
        // we cannot clear the swap now because it's possible that this future
        // will be canceled. In which case we should resume this swap the
        // next time this function is called
        if let Some(ref mut swap) = self.swap {
            // SAFETY: this swap is the latest swap
            unsafe { self.writer.afinish_swap(swap) }.await;
            // afinish_swap is driven to completion so now it's safe to clear the swap
            self.swap = None;
        }

        &mut self.writer
    }

    /// check if the writer is not in the middle of a swap
    ///
    /// if there is an in progress swap, then check that swap
    ///
    /// if there is no in progress swap, then return true
    #[inline]
    pub fn is_swap_finished(&mut self) -> bool {
        if let Some(ref mut swap) = self.swap {
            // SAFETY: This is the latest swap
            let b = unsafe { self.writer.is_swap_finished(swap) };
            if b {
                self.swap = None;
            }
            b
        } else {
            true
        }
    }

    /// check if there is an in progress swap
    #[inline]
    pub fn has_swap(&mut self) -> bool {
        self.swap.is_some()
    }

    /// try to get the underlying writer, but fails if there is a swap in progress
    ///
    /// Call [`Self::into_writer`] or [`Self::ainto_writer`]
    pub fn try_into_writer(self) -> Result<raw::Writer<P>, Self> {
        match self.swap {
            Some(_) => Err(self),
            None => Ok(self.writer),
        }
    }

    /// finish any ongoing swaps and get the underlying writer
    pub fn into_writer(mut self) -> raw::Writer<P>
    where
        P::Strategy: BlockingStrategy,
    {
        self.finish_swap();
        self.writer
    }

    /// finish any ongoing swaps and get the underlying writer
    pub async fn ainto_writer(mut self) -> raw::Writer<P>
    where
        P::Strategy: AsyncStrategy,
    {
        self.afinish_swap().await;
        self.writer
    }

    /// get the underlying writer, returns None if there is an ongoing swap
    pub fn get_writer_mut(&mut self) -> Option<&mut raw::Writer<P>> {
        self.is_swap_finished();
        match self.swap {
            Some(_) => None,
            None => Some(&mut self.writer),
        }
    }
}

impl<P: DoubleBufferWriterPointer> ops::Deref for DelayWriter<P> {
    type Target = raw::Writer<P>;

    fn deref(&self) -> &Self::Target {
        &self.writer
    }
}
