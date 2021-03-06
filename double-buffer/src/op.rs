#![forbid(unsafe_code)]

use crate::BufferRef;

use std::vec::Vec;

pub trait Operation<B>: Sized {
    fn apply(&mut self, buffer: &mut B);
    #[inline]
    fn apply_once(mut self, buffer: &mut B) { self.apply(buffer) }
}

pub struct Writer<B: BufferRef, O> {
    writer: crate::raw::Writer<B>,
    ops: Vec<O>,
}

pub struct WriterRef<'a, B: BufferRef, O> {
    buffer: &'a mut B::Buffer,
    ops: &'a mut Vec<O>,
}

impl<B: BufferRef, O> From<crate::raw::Writer<B>> for Writer<B, O> {
    #[inline]
    fn from(writer: crate::raw::Writer<B>) -> Self {
        Writer {
            writer,
            ops: Vec::new(),
        }
    }
}

impl<B: BufferRef, O> Writer<B, O> {
    pub fn reader(&self) -> crate::raw::Reader<B> { crate::raw::Writer::reader(&self.writer) }

    pub fn read(&self) -> &B::Buffer { crate::raw::Writer::read(&self.writer) }

    pub fn extra(&self) -> &B::Extra { crate::raw::Writer::extra(&self.writer) }

    #[inline]
    fn as_ref(&mut self) -> WriterRef<'_, B, O> {
        WriterRef {
            buffer: &mut self.writer,
            ops: &mut self.ops,
        }
    }
}

impl<B: BufferRef, O: Operation<B::Buffer>> Writer<B, O> {
    #[inline]
    pub fn split(&mut self) -> (&B::Buffer, WriterRef<'_, B, O>, &B::Extra) {
        let split = crate::raw::Writer::split_mut(&mut self.writer);
        (
            split.read,
            WriterRef {
                buffer: split.write,
                ops: &mut self.ops,
            },
            split.extra,
        )
    }

    #[inline]
    pub fn apply(&mut self, op: O) { self.as_ref().apply(op); }

    #[inline]
    pub fn apply_all<I: IntoIterator<Item = O>>(&mut self, ops: I) { self.as_ref().apply_all(ops); }

    #[cold]
    fn flush_slow(&mut self) {
        crate::raw::Writer::swap_buffers(&mut self.writer);
        let buffer = &mut self.writer as &mut B::Buffer;
        self.ops.drain(..).for_each(|op| op.apply_once(buffer))
    }

    #[inline]
    pub fn flush(&mut self) {
        if !self.ops.is_empty() {
            self.flush_slow();
        }
    }

    #[inline]
    pub fn operations(&self) -> &[O] { &self.ops }
}

impl<B: BufferRef, O: Operation<B::Buffer>> WriterRef<'_, B, O> {
    #[inline]
    pub fn apply(&mut self, mut op: O) {
        op.apply(self.buffer);
        self.ops.push(op);
    }

    #[inline]
    pub fn apply_all<I: IntoIterator<Item = O>>(&mut self, ops: I) {
        let buf: &mut B::Buffer = self.buffer;
        self.ops.extend(ops.into_iter().map(|mut op| {
            op.apply(buf);
            op
        }));
    }

    #[inline]
    pub fn operations(&self) -> &[O] { &self.ops }

    #[inline]
    pub fn by_ref(&mut self) -> WriterRef<'_, B, O> {
        WriterRef {
            buffer: self.buffer,
            ops: self.ops,
        }
    }
}
