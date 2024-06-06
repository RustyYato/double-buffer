use core::{
    alloc::Layout,
    iter::Flatten,
    marker::PhantomData,
    ptr::{self, NonNull},
    sync::atomic::Ordering,
};

use alloc::alloc::handle_alloc_error;
use crossbeam_utils::CachePadded;

#[cfg(not(loom))]
use core::sync::atomic::{AtomicBool, AtomicPtr};
#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, AtomicPtr};

type AtomicHazardPtr<T, const N: usize> = AtomicPtr<HazardNodeChunk<T, N>>;

pub struct Hazard<T, const N: usize> {
    head: AtomicHazardPtr<T, N>,
}

unsafe impl<T: Send, const N: usize> Send for Hazard<T, N> {}
unsafe impl<T: Send + Send, const N: usize> Sync for Hazard<T, N> {}

struct HazardNodeChunk<T, const N: usize> {
    next: AtomicHazardPtr<T, N>,
    items: [CachePadded<HazardNode<T, N>>; N],
}

struct HazardChunkIter<'a, T, const N: usize> {
    current: Option<NonNull<HazardNodeChunk<T, N>>>,
    lt: PhantomData<&'a HazardNodeChunk<T, N>>,
}

type HazardNodeIter<'a, T, const N: usize> = Flatten<HazardChunkIter<'a, T, N>>;

pub struct HazardIter<'a, T, const N: usize> {
    iter: HazardNodeIter<'a, T, N>,
}

struct HazardNode<T, const N: usize> {
    is_locked: AtomicBool,
    value: T,
}

pub struct RawHazardGuard<T, const N: usize> {
    node: NonNull<HazardNode<T, N>>,
}

unsafe impl<T: Send, const N: usize> Send for RawHazardGuard<T, N> {}
unsafe impl<T: Sync, const N: usize> Sync for RawHazardGuard<T, N> {}

impl<T, const N: usize> RawHazardGuard<T, N> {
    pub unsafe fn as_ref(&self) -> &T {
        unsafe { &(*self.node.as_ptr()).value }
    }
}

impl<T, const N: usize> Drop for RawHazardGuard<T, N> {
    fn drop(&mut self) {
        unsafe {
            &(*self.node.as_ptr())
                .is_locked
                .store(false, Ordering::Release);
        }
    }
}

impl<T, const N: usize> Drop for Hazard<T, N> {
    fn drop(&mut self) {
        #[cfg(not(loom))]
        let mut current_chunk = *self.head.get_mut();
        #[cfg(loom)]
        let mut current_chunk = self.head.with_mut(|x| *x);

        let layout = Layout::new::<HazardNodeChunk<T, N>>();
        while let Some(mut chunk) = NonNull::new(current_chunk) {
            #[cfg(not(loom))]
            {
                current_chunk = unsafe { *chunk.as_mut().next.get_mut() };
            }
            #[cfg(loom)]
            {
                current_chunk = unsafe { chunk.as_mut().next.with_mut(|x| *x) };
            }

            unsafe { alloc::alloc::dealloc(chunk.as_ptr().cast(), layout) }
        }
    }
}

impl<T, const N: usize> Hazard<T, N> {
    #[cfg(not(loom))]
    pub const fn new() -> Self {
        assert!(N != 0, "Cannot set batch size to zero");
        // since this is internal only, just assert that T doesn't need drop
        // to make Drop for Hazard<T, N> simpler
        const { assert!(!core::mem::needs_drop::<T>()) };
        Self {
            head: AtomicHazardPtr::new(ptr::null_mut()),
        }
    }

    #[cfg(loom)]
    pub fn new() -> Self {
        assert!(N != 0, "Cannot set batch size to zero");
        // since this is internal only, just assert that T doesn't need drop
        // to make Drop for Hazard<T, N> simpler
        const { assert!(!core::mem::needs_drop::<T>()) };
        Self {
            head: AtomicHazardPtr::new(ptr::null_mut()),
        }
    }

    pub fn get_or_insert_with(&self, f: impl FnMut() -> T) -> RawHazardGuard<T, N> {
        let node = self.nodes(Ordering::Acquire).find_map(|node| {
            // on single-threaded tests, the spurious failures are basically impossible
            // to hit. So just pretend that it's compare_exchange so MIRI won't
            // make tests unpredictable. We use loom to ensure that everything is working
            // in a MT environment anyways
            #[cfg(all(miri, test))]
            let is_success = node
                .is_locked
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok();

            #[cfg(not(all(miri, test)))]
            let is_success = node
                .is_locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok();

            if is_success {
                Some(RawHazardGuard {
                    node: NonNull::from(node as &HazardNode<T, N>),
                })
            } else {
                None
            }
        });

        if let Some(node) = node {
            return node;
        }

        self.insert_with(&mut { f })
    }

    #[cold]
    #[inline(never)]
    fn insert_with(&self, f: &mut dyn FnMut() -> T) -> RawHazardGuard<T, N> {
        let layout = Layout::new::<HazardNodeChunk<T, N>>();
        debug_assert_ne!(layout.size(), 0);
        let chunk = unsafe { alloc::alloc::alloc(layout) };

        let Some(chunk) = NonNull::new(chunk) else {
            handle_alloc_error(layout)
        };

        let chunk = chunk.cast::<HazardNodeChunk<T, N>>();

        unsafe {
            core::ptr::addr_of_mut!((*chunk.as_ptr()).next).write(AtomicPtr::new(ptr::null_mut()))
        }

        let items = unsafe { core::ptr::addr_of_mut!((*chunk.as_ptr()).items) };
        let items: *mut CachePadded<HazardNode<T, N>> = items.cast();

        for i in 0..N {
            let node: *mut HazardNode<T, N> = unsafe { items.add(i).cast() };

            unsafe {
                node.write(HazardNode {
                    is_locked: AtomicBool::new(false),
                    value: f(),
                })
            }
        }

        // lock the first item on creation
        let first_node: *mut HazardNode<T, N> = items.cast();
        unsafe { (*first_node).is_locked = AtomicBool::new(true) }

        self.head
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |prev_head| unsafe {
                (*chunk.as_ptr()).next = AtomicPtr::new(prev_head);
                Some(chunk.as_ptr())
            })
            .unwrap();

        RawHazardGuard {
            node: unsafe { NonNull::new_unchecked(first_node) },
        }
    }

    fn chunks(&self, order: Ordering) -> HazardChunkIter<'_, T, N> {
        HazardChunkIter {
            current: NonNull::new(self.head.load(order)),
            lt: PhantomData,
        }
    }

    fn nodes(&self, order: Ordering) -> HazardNodeIter<'_, T, N> {
        self.chunks(order).flatten()
    }

    pub fn iter(&self) -> HazardIter<'_, T, N> {
        HazardIter {
            iter: self.nodes(Ordering::Relaxed),
        }
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a HazardNodeChunk<T, N> {
    type Item = &'a CachePadded<HazardNode<T, N>>;
    type IntoIter = core::slice::Iter<'a, CachePadded<HazardNode<T, N>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl<'a, T, const N: usize> Iterator for HazardChunkIter<'a, T, N> {
    type Item = &'a HazardNodeChunk<T, N>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = unsafe { self.current?.as_ref() };
        self.current = NonNull::new(current.next.load(Ordering::Relaxed));
        Some(current)
    }
}

impl<'a, T, const N: usize> Iterator for HazardIter<'a, T, N> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|x| &x.value)
    }
}

#[cfg(test)]
fn assert_no_dups<T: Ord, const N: usize>(mut ts: [T; N]) {
    ts.sort_unstable();
    assert!(ts.windows(2).all(|x| x[0] != x[1]));
}

#[cfg(not(loom))]
#[test]
fn test_basic() {
    let hazard = Hazard::<i32, 4>::new();

    let a = hazard.get_or_insert_with(|| 0);
    let b = hazard.get_or_insert_with(|| panic!());
    let c = hazard.get_or_insert_with(|| panic!());
    let d = hazard.get_or_insert_with(|| panic!());

    assert_no_dups([a.node, b.node, c.node, d.node]);

    let e = hazard.get_or_insert_with(|| 1);

    assert_no_dups([a.node, b.node, c.node, d.node, e.node]);

    unsafe {
        assert_eq!(*a.as_ref(), 0);
        assert_eq!(*b.as_ref(), 0);
        assert_eq!(*c.as_ref(), 0);
        assert_eq!(*d.as_ref(), 0);
        assert_eq!(*e.as_ref(), 1);
    }

    drop(b);
    let f = hazard.get_or_insert_with(|| panic!());
    let g = hazard.get_or_insert_with(|| panic!());
    let h = hazard.get_or_insert_with(|| panic!());
    let i = hazard.get_or_insert_with(|| panic!());

    unsafe {
        assert_eq!(*a.as_ref(), 0);
        assert_eq!(*c.as_ref(), 0);
        assert_eq!(*d.as_ref(), 0);
        assert_eq!(*e.as_ref(), 1);
        assert_eq!(*f.as_ref(), 1);
        assert_eq!(*g.as_ref(), 1);
        assert_eq!(*h.as_ref(), 1);
        assert_eq!(*i.as_ref(), 0);
    }

    drop((a, c, d));

    let j = hazard.get_or_insert_with(|| panic!());
    let k = hazard.get_or_insert_with(|| panic!());
    let l = hazard.get_or_insert_with(|| panic!());

    unsafe {
        assert_eq!(*e.as_ref(), 1);
        assert_eq!(*f.as_ref(), 1);
        assert_eq!(*g.as_ref(), 1);
        assert_eq!(*h.as_ref(), 1);
        assert_eq!(*i.as_ref(), 0);
        assert_eq!(*j.as_ref(), 0);
        assert_eq!(*k.as_ref(), 0);
        assert_eq!(*l.as_ref(), 0);
    }
}

#[cfg(loom)]
#[test]
fn test_loom_simple() {
    loom::model(|| {
        let hazard = Hazard::<u32, 1>::new();
        let hazard = loom::sync::Arc::new(hazard);

        let mut threads = alloc::vec::Vec::new();

        for i in 0..3 {
            let hazard = hazard.clone();
            let t = loom::thread::spawn(move || {
                let x = hazard.get_or_insert_with(|| i);
                assert!((0..3).contains(unsafe { x.as_ref() }))
            });

            threads.push(t);
        }

        for t in threads {
            t.join().unwrap();
        }

        let a = hazard.get_or_insert_with(|| 3);
        let b = hazard.get_or_insert_with(|| 3);
        let c = hazard.get_or_insert_with(|| 3);

        assert!((0..4).contains(unsafe { a.as_ref() }));
        assert!((0..4).contains(unsafe { b.as_ref() }));
        assert!((0..4).contains(unsafe { c.as_ref() }));

        // all items are either < 3 and unique or == 3
        let or = |x: &u32, y: u32| if *x == 3 { y } else { *x };
        let mut i = 5;
        unsafe {
            assert_no_dups([a.as_ref(), b.as_ref(), c.as_ref()].map(|x| {
                i += 1;
                or(x, i)
            }))
        }
    })
}

#[cfg(loom)]
#[test]
fn test_loom_chunked() {
    loom::model(|| {
        let hazard = Hazard::<u32, 3>::new();
        let hazard = loom::sync::Arc::new(hazard);

        let mut threads = alloc::vec::Vec::new();

        for i in 0..3 {
            let hazard = hazard.clone();
            let t = loom::thread::spawn(move || {
                let x = hazard.get_or_insert_with(|| i);
                assert!((0..3).contains(unsafe { x.as_ref() }))
            });

            threads.push(t);
        }

        for t in threads {
            t.join().unwrap();
        }

        let a = hazard.get_or_insert_with(|| panic!());
        let b = hazard.get_or_insert_with(|| panic!());
        let c = hazard.get_or_insert_with(|| panic!());

        assert!((0..3).contains(unsafe { a.as_ref() }));
        assert!((0..3).contains(unsafe { b.as_ref() }));
        assert!((0..3).contains(unsafe { c.as_ref() }));

        unsafe {
            assert_eq!(a.as_ref(), b.as_ref());
            assert_eq!(a.as_ref(), c.as_ref());
        }
    })
}
