use std::ptr::NonNull;

#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(loom))]
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SplitArc<T: ?Sized> {
    ptr: NonNull<SplitArcInner<T>>,
}

struct SplitArcInner<T: ?Sized> {
    is_split: AtomicBool,
    data: T,
}

unsafe impl<T: Send + Sync> Send for SplitArc<T> {}
unsafe impl<T: Send + Sync> Sync for SplitArc<T> {}

impl<T> SplitArc<T> {
    pub fn new(data: T) -> Self {
        Self {
            ptr: unsafe {
                NonNull::new_unchecked(Box::into_raw(Box::new(SplitArcInner {
                    is_split: AtomicBool::new(false),
                    data,
                })))
            },
        }
    }
}

impl<T: ?Sized> SplitArc<T> {
    pub fn try_split(&self) -> Option<Self> {
        let info = unsafe { self.ptr.as_ref() };

        let is_split = info
            .is_split
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok();

        if is_split {
            Some(Self { ptr: self.ptr })
        } else {
            None
        }
    }

    pub fn split(&self) -> Self {
        self.try_split().expect("This `SplitArc` is already split")
    }

    fn drop_ref(&self) -> bool {
        let info = unsafe { self.ptr.as_ref() };

        info.is_split
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    }
}

impl<T> core::ops::Deref for SplitArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.ptr.as_ref().data }
    }
}

impl<T: ?Sized> Drop for SplitArc<T> {
    fn drop(&mut self) {
        if self.drop_ref() {
            unsafe {
                let _ = Box::from_raw(self.ptr.as_ptr());
            }
        }
    }
}

#[test]
#[cfg(loom)]
fn test() {
    struct Foo(loom::cell::UnsafeCell<i32>);

    impl Drop for Foo {
        fn drop(&mut self) {
            self.0.with_mut(|_| loom::thread::yield_now())
        }
    }

    loom::model(|| {
        let arc = SplitArc::new(Foo(loom::cell::UnsafeCell::new(10)));

        let b = arc.split();
        loom::thread::spawn(|| {
            let _c = b.try_split();
            drop(b);
        });

        drop(arc);
    })
}
