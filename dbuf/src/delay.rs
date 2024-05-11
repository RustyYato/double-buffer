use core::fmt::Debug;
use core::ops;

use crate::{
    interface::{AsyncStrategy, BlockingStrategy, DoubleBufferWriterPointer, Strategy, SwapError},
    raw,
};

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
    pub const fn from_writer(writer: raw::Writer<P>) -> Self {
        Self { writer, swap: None }
    }

    pub fn try_start_swap(&mut self) -> Result<(), SwapError<P::Strategy>> {
        if self.swap.is_none() {
            // SAFETY: DelayWriter ensures that finish_swap is called before allowing
            // mutable access to the writer
            self.swap = Some(unsafe { self.writer.try_start_swap()? })
        }

        Ok(())
    }

    pub fn start_swap(&mut self)
    where
        SwapError<P::Strategy>: Debug,
    {
        self.try_start_swap().expect("start stop must not fail")
    }

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

    pub async fn afinish_swap(&mut self) -> &mut raw::Writer<P>
    where
        P::Strategy: AsyncStrategy,
    {
        // we cannot clear the swap now because it's possible that this future
        // will be cancelled. In which case we should resume this swap the
        // next time this function is called
        if let Some(ref mut swap) = self.swap {
            // SAFETY: this swap is the latest swap
            unsafe { self.writer.try_afinish_swap(swap) }.await;
            // afinish_swap is driven to completion so now it's safe to clear the swap
            self.swap = None;
        }

        &mut self.writer
    }

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

    #[inline]
    pub fn has_swap(&mut self) -> bool {
        self.swap.is_some()
    }

    pub fn try_into_writer(self) -> Result<raw::Writer<P>, Self> {
        match self.swap {
            Some(_) => Err(self),
            None => Ok(self.writer),
        }
    }

    pub fn into_writer(mut self) -> raw::Writer<P>
    where
        P::Strategy: BlockingStrategy,
    {
        self.finish_swap();
        self.writer
    }

    pub async fn ainto_writer(mut self) -> raw::Writer<P>
    where
        P::Strategy: AsyncStrategy,
    {
        self.afinish_swap().await;
        self.writer
    }

    pub fn get_writer_mut(&mut self) -> Option<&mut raw::Writer<P>> {
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
