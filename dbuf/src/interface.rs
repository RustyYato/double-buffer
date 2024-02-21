use core::{ops, task::Context};

use crate::raw::Cow;

pub(crate) type WriterId<S> = <S as Strategy>::WriterId;
pub(crate) type ReaderId<S> = <S as Strategy>::ReaderId;
pub(crate) type ReaderGuard<S> = <S as Strategy>::ReadGuard;
pub(crate) type Swap<S> = <S as Strategy>::Swap;
pub(crate) type SwapError<S> = <S as Strategy>::SwapError;

/// Convert a pointer into a writer pointer
///
/// # Safety
///
/// Self::deref, Self::deref_mut, Self::Writer::deref, Self::Writer::deref_mut
/// must all point to the same [`DoubleBufferData`]
///
/// the writer produced by into_writer must not be aliased
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

/// A pointer that can access a and may hold ownership over a [`DoubleBufferData`]
///
/// # Safety
///
/// Self::deref  must not change which [`DoubleBufferData`] it points to.
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

/// A pointer doesn't usually doesn't hold ownership over a [`DoubleBufferData`],
/// but may be converted to a [`DoubleBufferWriterPointer`] to access it.
///
/// # Safety
///
/// as long as the only usage of this type is through try_writer;
/// * multiple calls to try_writer must yield the same writer
/// * once try_writer returns [`Err`], it must never return [`Ok`] again
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

/// The syncronization strategy of the double buffer
///
/// # Safety
///
/// first some terminology, there is an active read during the time
/// between a call to [`acquire_read_guard`] and [`release_read_guard`]
///
/// * finish_swap must not return if there is an active read
pub unsafe trait Strategy {
    type WriterId;
    type ReaderId;

    type Swap;
    type SwapError;

    type ReadGuard;

    // id constructors

    /// Creates a valid writer id for this strategy, and invalidates all writer ids
    /// and reader ids created by this strategy before this call to [`create_writer_id`].
    fn create_writer_id(&mut self) -> Self::WriterId;

    /// Creates a valid reader id from the provided writer id
    ///
    /// # Safety
    ///
    /// The writer id must be valid for this strategy
    unsafe fn create_reader_id_from_writer(&self, writer: &Self::WriterId) -> Self::ReaderId;

    /// Creates a valid reader id from the provided reader id
    ///
    /// # Safety
    ///
    /// The reader id
    unsafe fn create_reader_id_from_reader(&self, reader: &Self::ReaderId) -> Self::ReaderId;

    /// Creates an invalid reader id
    ///
    /// This is useful if you need *some* reader id, but it won't be used by anyone
    fn create_invalid_reader_id() -> Self::ReaderId;

    // accessors

    /// Returns true if the number of successful calls to try_start_swap is odd
    ///
    /// May only be called from the writer
    ///
    /// # Safety
    ///
    /// The writer id must be valid
    unsafe fn is_swapped_writer(&self, writer: &Self::WriterId) -> bool;

    /// Returns true if the number of successful calls to try_start_swap is odd
    ///
    /// May only be called from the reader once a guard is taken
    ///
    /// # Safety
    ///
    /// * The reader id is valid
    /// * The reader guard must have been created from the given reader id
    unsafe fn is_swapped(&self, reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool;

    // swap handlers

    /// Tries to start a swap
    ///
    /// If the buffers can be swapped without issues, then they will be swapped
    /// and this function will return Ok
    /// otherwise this function will return Err (and the buffers will not be swapped)
    ///
    /// NOTE: for implementors, it is safe to call try_start_swap as many times as the
    /// user wants, and only the last swap matters.
    ///
    /// # Safety
    ///
    /// the writer id must be valid
    unsafe fn try_start_swap(
        &self,
        writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError>;

    /// Returns true if the latest swap is finished
    ///
    /// # Safety
    ///
    /// the writer id must be valid
    /// the swap must have been created by this strategy and this writer id
    /// this must be the latest swap created by this strategy and writer id
    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool;

    // reader registration

    /// Acquires a read guard. This ensures that the writer does not have write access to the
    /// current buffer while the read guard is active
    ///
    /// NOTE: it is incorrect, but not *unsafe* to call [`acquire_read_guard`] while there is
    /// an unreleased [`Self::ReadGuard`]. This can result in a panic, infinite loop, or any other
    /// strange but safe behavior.
    ///
    /// # Safety
    ///
    /// The reader id must be valid
    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard;

    /// Release a read guard. This allows the writer to write the buffer that this guard protects
    /// if there are no other active reads to that buffer
    ///
    /// # Safety
    ///
    /// The reader id must be valid
    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, guard: Self::ReadGuard);
}

pub trait AsyncStrategy: Strategy {
    /// registers a async context to an ongoing swap
    ///
    /// # Safety
    ///
    /// the writer id must be valid
    /// the swap must have been created by this strategy and this writer id
    /// this must be the latest swap created by this strategy and writer id
    unsafe fn register_context(
        &self,
        writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut Context<'_>,
    ) -> core::task::Poll<()>;
}

pub trait BlockingStrategy: Strategy {
    /// Waits until the latest swap is finished
    ///
    /// If is_swap_finished returned true before this is called (with the same arguments)
    /// then this function will return without blocking.
    ///
    /// # Safety
    ///
    /// the writer id must be valid
    /// the swap must have been created by this strategy and this writer id
    /// this must be the latest swap created by this strategy and writer id
    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, swap: Self::Swap);
}

pub(crate) fn create_invalid_reader_id<S: Strategy>() -> S::ReaderId {
    S::create_invalid_reader_id()
}
