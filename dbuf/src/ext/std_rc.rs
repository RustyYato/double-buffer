use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::{Cow, DoubleBufferData},
};

use alloc::rc::{Rc, Weak};
use rc_box::RcBox;

// SAFETY: RcBox is guaranteed to not be aliased
// and will point to the same value as the Arc created from RcBox::into
unsafe impl<T, S: Strategy, Extras> IntoDoubleBufferWriterPointer
    for RcBox<DoubleBufferData<T, S, Extras>>
{
    type Writer = Rc<DoubleBufferData<T, S, Extras>>;

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
    for Rc<DoubleBufferData<T, S, Extras>>
{
    type Reader = Weak<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    #[inline]
    fn reader(&self) -> Self::Reader {
        Rc::downgrade(self)
    }
}

// SAFETY: as long as the only usage of this type is through try_writer;
// * multiple calls to try_writer must yield the same writer
//  Self::upgrade always yields the same Rc
// * once try_writer returns [`Err`], it must never return [`Ok`] again
//  Self::upgrade will only return None if there are no more Rcs
//  and it's not possible to revive an Rc from a Weak after it has been destroyed
unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferReaderPointer
    for Weak<DoubleBufferData<T, S, Extras>>
{
    type Writer = Rc<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    type UpgradeError = RcUpgradeError;

    #[inline]
    fn try_writer(&self) -> Result<Cow<'_, Self::Writer>, Self::UpgradeError> {
        self.upgrade().map(Cow::Owned).ok_or(RcUpgradeError)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RcUpgradeError;

impl core::fmt::Debug for RcUpgradeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Cannot upgrade a dangling weak to an Rc")
    }
}
