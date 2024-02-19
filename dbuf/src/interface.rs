use core::{ops, task::Context};

use crate::raw::Cow;

pub(crate) type WriterId<S> = <S as Strategy>::WriterId;
pub(crate) type ReaderId<S> = <S as Strategy>::ReaderId;
pub(crate) type ReaderGuard<S> = <S as Strategy>::ReadGuard;
pub(crate) type Swap<S> = <S as Strategy>::Swap;
pub(crate) type SwapError<S> = <S as Strategy>::SwapError;

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
    type SwapError;

    type ReadGuard;

    // id constructors

    unsafe fn create_writer_id(&mut self) -> Self::WriterId;

    unsafe fn create_reader_id_from_writer(&self, writer: &Self::WriterId) -> Self::ReaderId;

    unsafe fn create_reader_id_from_reader(&self, reader: &Self::ReaderId) -> Self::ReaderId;

    unsafe fn create_invalid_reader_id() -> Self::ReaderId;

    // accessors

    unsafe fn is_swapped_exclusive(&self, writer: &mut Self::WriterId) -> bool;

    unsafe fn is_swapped_shared(&self, writer: &Self::WriterId) -> bool;

    unsafe fn is_swapped(&self, guard: &Self::ReadGuard) -> bool;

    // swap handlers

    unsafe fn try_start_swap(
        &self,
        writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError>;

    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool;

    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, swap: Self::Swap);

    // reader registration

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard;

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, guard: Self::ReadGuard);
}

pub unsafe trait AsyncStrategy: Strategy {
    unsafe fn register_context(
        &self,
        writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut Context<'_>,
    );
}

pub(crate) unsafe fn create_invalid_reader_id<S: Strategy>() -> S::ReaderId {
    S::create_invalid_reader_id()
}
