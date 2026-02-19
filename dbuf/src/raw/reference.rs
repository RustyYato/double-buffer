use core::{marker::PhantomData, ops::Deref, ptr::NonNull};

pub(crate) struct RawReference<'a, T: ?Sized> {
    ptr: NonNull<T>,
    lt: PhantomData<&'a T>,
}
/// SAFETY: [`RawReference`] is semantically equivalent to a [`&T`] but without
/// the validity requirements
unsafe impl<T: ?Sized> Send for RawReference<'_, T> where T: Sync {}
/// SAFETY: [`RawReference`] is semantically equivalent to a [`&T`] but without
/// the validity requirements
unsafe impl<T: ?Sized> Sync for RawReference<'_, T> where T: Sync {}
impl<T: ?Sized> core::panic::UnwindSafe for RawReference<'_, T> where T: core::panic::RefUnwindSafe {}
impl<T: ?Sized> core::panic::RefUnwindSafe for RawReference<'_, T> where
    T: core::panic::RefUnwindSafe
{
}

impl<T: ?Sized> RawReference<'_, T> {
    /// # Safety
    ///
    /// the pointer is non-null and valid to convert to a reference for the lifetime bound to the `RawReference`
    pub(crate) const unsafe fn new(ptr: *mut T) -> Self {
        Self {
            // SAFETY: the caller ensure that the pointer is non-null
            ptr: unsafe { NonNull::new_unchecked(ptr) },
            lt: PhantomData,
        }
    }

    pub const fn as_ref(&self) -> &T {
        // SAFETY: the creator of `RawReference` ensure that the pointer is valid to convert to a reference
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> Deref for RawReference<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<'a: 'b, 'b, T: ?Sized> From<&'a T> for RawReference<'b, T> {
    fn from(value: &'a T) -> Self {
        RawReference {
            ptr: NonNull::from(value),
            lt: PhantomData,
        }
    }
}
