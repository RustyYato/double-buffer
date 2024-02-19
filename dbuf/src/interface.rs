use core::ops;

use alloc::borrow::Cow;

pub(crate) type WriterId<S> = <S as Strategy>::WriterId;
pub(crate) type ReaderId<S> = <S as Strategy>::ReaderId;

pub unsafe trait IntoDoubleBufferWriterPointer:
    ops::DerefMut<Target = crate::raw::DoubleBufferData<Self::Buffer, Self::Strategy, Self::Extras>>
{
    type Writer: DoubleBufferWriterPointer<
        Strategy = Self::Strategy,
        Buffer = Self::Buffer,
        Extras = Self::Extras,
    >;

    type Strategy: Strategy;
    type Buffer;
    type Extras: ?Sized;

    fn into_writer(self) -> Self::Writer;
}

pub unsafe trait DoubleBufferWriterPointer:
    Clone
    + ops::Deref<Target = crate::raw::DoubleBufferData<Self::Buffer, Self::Strategy, Self::Extras>>
{
    type Reader: DoubleBufferReaderPointer<
        Writer = Self,
        Strategy = Self::Strategy,
        Buffer = Self::Buffer,
        Extras = Self::Extras,
    >;

    type Strategy: Strategy;
    type Buffer;
    type Extras: ?Sized;

    fn reader(&self) -> Self::Reader;
}

pub unsafe trait DoubleBufferReaderPointer: Clone {
    type Writer: DoubleBufferWriterPointer<
        Reader = Self,
        Strategy = Self::Strategy,
        Buffer = Self::Buffer,
        Extras = Self::Extras,
    >;
    type UpgradeError;

    type Strategy: Strategy;
    type Buffer;
    type Extras: ?Sized;

    fn try_writer(&self) -> Result<Cow<'_, Self::Writer>, Self::UpgradeError>;
}

pub unsafe trait Strategy {
    type WriterId;
    type ReaderId;

    type Swap;

    type ReadGuard;

    // id constructors

    unsafe fn create_writer_id(&mut self) -> Self::WriterId;

    unsafe fn create_reader_id_from_writer(&self, writer: &Self::WriterId) -> Self::ReaderId;

    unsafe fn create_reader_id_from_reader(&self, reader: &Self::ReaderId) -> Self::ReaderId;

    unsafe fn create_invalid_reader_id() -> Self::ReaderId;

    // swap handlers

    unsafe fn start_swap(&self, writer: &mut Self::WriterId) -> Self::Swap;

    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool;

    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, swap: Self::Swap);

    // reader registration

    unsafe fn acquire_read_guard(&self, reader: &Self::ReaderId) -> Self::ReadGuard;

    unsafe fn release_read_guard(&self, reader: &Self::ReaderId, guard: Self::ReadGuard);
}

pub(crate) unsafe fn create_invalid_reader_id<S: Strategy>() -> S::ReaderId {
    S::create_invalid_reader_id()
}
