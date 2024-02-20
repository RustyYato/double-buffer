#![forbid(unsafe_code)]

use crate::{
    delay::DelayWriter,
    interface::{DoubleBufferWriterPointer, Strategy},
};

use alloc::vec::Vec;

pub struct OpWriter<
    P: DoubleBufferWriterPointer,
    O,
    S: Strategy = <P as DoubleBufferWriterPointer>::Strategy,
> {
    writer: DelayWriter<P, S>,
    op_log: Vec<O>,
    water_line: usize,
}

pub trait Operation<T: ?Sized>: Sized {
    fn apply(&mut self, buffer: &mut T);

    fn apply_once(mut self, buffer: &mut T) {
        self.apply(buffer)
    }
}

impl<P: DoubleBufferWriterPointer, O: Operation<P::Buffer>> OpWriter<P, O> {
    pub fn from_writer(writer: DelayWriter<P>) -> Self {
        Self {
            writer,
            op_log: Vec::new(),
            water_line: 0,
        }
    }

    pub fn swap_buffers(&mut self)
    where
        P::Strategy: Strategy<SwapError = core::convert::Infallible>,
    {
        let writer = self.writer.finish_swap();

        let buffer = writer.get_mut();

        for op in self.op_log.drain(..self.water_line) {
            op.apply_once(buffer);
        }

        for op in self.op_log.iter_mut() {
            op.apply(buffer);
        }

        self.writer.start_swap()
    }

    #[inline]
    pub fn push(&mut self, op: O) {
        self.op_log.push(op)
    }

    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.op_log.reserve(additional)
    }
}

impl<P: DoubleBufferWriterPointer, O: Operation<P::Buffer>> Extend<O> for OpWriter<P, O> {
    #[inline]
    fn extend<T: IntoIterator<Item = O>>(&mut self, iter: T) {
        self.op_log.extend(iter)
    }
}
