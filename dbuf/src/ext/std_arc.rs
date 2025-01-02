use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::DoubleBufferData,
};

use alloc::sync::{Arc, Weak};
use rc_box::ArcBox;

// SAFETY: ArcBox is guaranteed to not be aliased
// and will point to the same value as the Arc created from ArcBox::into
unsafe impl<T, S: Strategy, Extras> IntoDoubleBufferWriterPointer
    for ArcBox<DoubleBufferData<T, S, Extras>>
{
    type Writer = Arc<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    fn into_writer(self) -> Self::Writer {
        self.into()
    }
}

// SAFETY: Self::deref does not change which [`DoubleBufferData`] it points to
// Self::reader -> try_reader will return self
unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferWriterPointer
    for Arc<DoubleBufferData<T, S, Extras>>
{
    type Reader = Weak<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    #[inline]
    fn reader(&self) -> Self::Reader {
        Arc::downgrade(self)
    }
}

// SAFETY: as long as the only usage of this type is through try_writer;
// * multiple calls to try_writer must yield the same writer
//  Self::upgrade always yields the same Arc
// * once try_writer returns [`Err`], it must never return [`Ok`] again
//  Self::upgrade will only return None if there are no more Arcs
//  and it's not possible to revive an Arc from a Weak after it has been destroyed
unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferReaderPointer
    for Weak<DoubleBufferData<T, S, Extras>>
{
    type Writer = Arc<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    type UpgradeError = ArcUpgradeError;
    type MaybeBorrowed<'a>
        = Self::Writer
    where
        Self: 'a;

    #[inline]
    fn try_writer(&self) -> Result<Self::MaybeBorrowed<'_>, Self::UpgradeError> {
        self.upgrade().ok_or(ArcUpgradeError)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ArcUpgradeError;

impl core::fmt::Debug for ArcUpgradeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Cannot upgrade a dangling weak to an Rc")
    }
}
