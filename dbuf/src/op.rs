#![forbid(unsafe_code)]

use crate::{
    delay::DelayWriter,
    interface::{AsyncStrategy, BlockingStrategy, DoubleBufferWriterPointer, Strategy},
    raw,
};

use alloc::vec::Vec;
use sync_wrapper::SyncWrapper;

pub struct OpWriter<
    P: DoubleBufferWriterPointer,
    O,
    S: Strategy = <P as DoubleBufferWriterPointer>::Strategy,
> {
    writer: DelayWriter<P, S>,
    op_log: Vec<sync_wrapper::SyncWrapper<O>>,
    water_line: usize,
}

pub trait Operation<T: ?Sized, E: ?Sized, P: ?Sized>: Sized {
    fn apply(&mut self, buffer: &mut T, extra: &E, params: &mut P);

    fn apply_once(mut self, buffer: &mut T, extra: &E, params: &mut P) {
        self.apply(buffer, extra, params)
    }
}

impl<P: DoubleBufferWriterPointer, O> From<raw::Writer<P>> for OpWriter<P, O> {
    fn from(writer: raw::Writer<P>) -> Self {
        Self::from_writer(writer.into())
    }
}

impl<P: DoubleBufferWriterPointer, O> From<DelayWriter<P>> for OpWriter<P, O> {
    fn from(writer: DelayWriter<P>) -> Self {
        Self::from_writer(writer)
    }
}

impl<P: DoubleBufferWriterPointer, O> OpWriter<P, O> {
    pub fn from_writer(writer: DelayWriter<P>) -> Self {
        Self {
            writer,
            op_log: Vec::new(),
            water_line: 0,
        }
    }

    pub fn swap_buffers<Params: ?Sized>(&mut self, params: &mut Params)
    where
        P::Strategy: BlockingStrategy + Strategy<SwapError = core::convert::Infallible>,
        O: Operation<P::Buffer, P::Extras, Params>,
    {
        let writer = self.writer.finish_swap();
        swap_buffers(writer, &mut self.op_log, &mut self.water_line, params);
        self.writer.start_swap();
    }

    pub async fn aswap_buffers<Params: ?Sized>(&mut self, params: &mut Params)
    where
        P::Strategy: AsyncStrategy + Strategy<SwapError = core::convert::Infallible>,
        O: Operation<P::Buffer, P::Extras, Params>,
    {
        let writer = self.writer.afinish_swap().await;

        swap_buffers(writer, &mut self.op_log, &mut self.water_line, params);
        self.writer.start_swap();
    }

    #[inline]
    pub fn push(&mut self, op: O) {
        self.op_log.push(SyncWrapper::new(op))
    }

    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.op_log.reserve(additional)
    }
}

impl<P: DoubleBufferWriterPointer, O> core::ops::Deref for OpWriter<P, O> {
    type Target = crate::raw::Writer<P>;

    fn deref(&self) -> &Self::Target {
        &self.writer
    }
}

impl<P: DoubleBufferWriterPointer, O> Extend<O> for OpWriter<P, O> {
    #[inline]
    fn extend<T: IntoIterator<Item = O>>(&mut self, iter: T) {
        self.op_log.extend(iter.into_iter().map(SyncWrapper::new))
    }
}

fn swap_buffers<
    P: DoubleBufferWriterPointer,
    O: Operation<P::Buffer, P::Extras, Params>,
    Params: ?Sized,
>(
    writer: &mut raw::Writer<P>,
    op_log: &mut Vec<sync_wrapper::SyncWrapper<O>>,
    water_line: &mut usize,
    params: &mut Params,
) where
    P::Strategy: Strategy<SwapError = core::convert::Infallible>,
{
    let split = writer.split_mut();
    let buffer = split.write;
    let extras = split.extras;

    let water_line = &mut SetOnDrop::new(water_line).0;
    #[allow(clippy::arithmetic_side_effects)]
    for op in crate::vec_drain::drain_unti(op_log, ..*water_line) {
        *water_line -= 1;
        op.into_inner().apply_once(buffer, extras, params);
    }

    for op in op_log.iter_mut() {
        op.get_mut().apply(buffer, extras, params);
    }
}

struct SetOnDrop<'a>(usize, &'a mut usize);

impl<'a> SetOnDrop<'a> {
    pub fn new(value: &'a mut usize) -> Self {
        Self(*value, value)
    }
}

impl Drop for SetOnDrop<'_> {
    #[inline]
    fn drop(&mut self) {
        *self.1 = self.0
    }
}
