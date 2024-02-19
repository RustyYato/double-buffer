use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::{Cow, DoubleBufferData},
};

use alloc::rc::{Rc, Weak};
use rc_box::RcBox;

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
