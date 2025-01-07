#[macro_export]
macro_rules! static_once {
    ($ty:ty => const $value:block) => {{
        static mut VALUE: $ty = $value;
        static ONCE: $crate::macros::AtomicBool = $crate::macros::AtomicBool::new(false);

        if ONCE
            .compare_exchange(
                false,
                true,
                $crate::macros::Relaxed,
                $crate::macros::Relaxed,
            )
            .is_ok()
        {
            #[allow(static_mut_refs)]
            // SAFETY: This value is only accessed once
            Some(unsafe { &mut VALUE })
        } else {
            None
        }
    }};
    ($ty:ty => $value:expr) => {{
        static mut VALUE: $crate::macros::MaybeUninit<$ty> = $crate::macros::MaybeUninit::uninit();
        static ONCE: $crate::macros::AtomicBool = $crate::macros::AtomicBool::new(false);

        if ONCE
            .compare_exchange(
                false,
                true,
                $crate::macros::Relaxed,
                $crate::macros::Relaxed,
            )
            .is_ok()
        {
            match $value {
                // SAFETY: This value is only accessed once
                value => unsafe { VALUE = $crate::macros::MaybeUninit::new(value) },
            }
            #[allow(static_mut_refs)]
            // SAFETY: This value is only accessed once
            Some(unsafe { &mut *VALUE.as_mut_ptr() })
        } else {
            None
        }
    }};
}

pub use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};

#[test]
fn test() {
    for x in 0..100 {
        let once = static_once!(() => const {});
        assert_eq!(once.is_some(), x == 0)
    }

    for x in 0..100 {
        let once = static_once!(() => ());
        assert_eq!(once.is_some(), x == 0)
    }
}

#[test]
#[cfg(feature = "std")]
#[cfg_attr(miri, ignore)]
fn test_mt() {
    let counter = core::sync::atomic::AtomicU32::new(0);
    std::thread::scope(|s| {
        for _ in 0..100 {
            s.spawn(|| {
                let val = static_once!(() => const {}).is_some();
                counter.fetch_add(val as u32, Relaxed);
            });
        }
    });
    assert_eq!(counter.load(Relaxed), 1);
}
