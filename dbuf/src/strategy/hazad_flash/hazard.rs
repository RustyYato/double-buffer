use core::{
    alloc::Layout,
    iter::Flatten,
    marker::PhantomData,
    num::NonZeroUsize,
    ptr::{self, NonNull},
    sync::atomic::Ordering,
};

use alloc::alloc::handle_alloc_error;
use const_fn::const_fn;
use crossbeam_utils::CachePadded;

#[cfg(not(loom))]
use core::sync::atomic::{AtomicBool, AtomicPtr};
#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, AtomicPtr};

type AtomicHazardPtr<T, const N: usize> = AtomicPtr<HazardNodeChunk<T, N>>;

pub struct Hazard<T, const N: usize> {
    head: AtomicHazardPtr<T, N>,
}

/// SAFETY: Hazard drops all T's, so Hazard: Send only if T: Send
unsafe impl<T: Send, const N: usize> Send for Hazard<T, N> {}
/// SAFETY: Hazard exposes get_or_insert_with, which is &self -> &T, so
/// Hazard: Sync only if T: Sync
/// Hazard doesn't expose any functions &self -> &mut T, so T: Send isn't required
unsafe impl<T: Send, const N: usize> Sync for Hazard<T, N> {}

#[repr(align(8))]
struct HazardNodeChunk<T, const N: usize> {
    // This ptr is only written to while HazardNodeChunk isn't shared
    // so it doesn't need to be atomic
    next: *mut HazardNodeChunk<T, N>,
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
    // This is a tagged pointer,
    node: NonNull<HazardNode<T, N>>,
}

/// SAFETY: `RawHazardGuard` is a just like &T so it has the same requirements
unsafe impl<T: Sync, const N: usize> Send for RawHazardGuard<T, N> {}
/// SAFETY: `RawHazardGuard` is a just like &T so it has the same requirements
unsafe impl<T: Sync, const N: usize> Sync for RawHazardGuard<T, N> {}

fn addr<T>(x: NonNull<T>) -> NonZeroUsize {
    // SAFETY: transmuting from a pointer to a usize is safe
    // it just drops the provenance of the pointer
    unsafe { core::mem::transmute(x) }
}

fn with_addr<T>(x: NonNull<T>, addr: NonZeroUsize) -> NonNull<T> {
    let ptr = x.as_ptr();
    let ptr = ptr
        .wrapping_byte_sub(self::addr(x).get())
        .wrapping_byte_add(addr.get());

    // SAFETY: ^^^ sets the pointer's address to addr, which is non-zero
    // so the pointer is non-null
    unsafe { NonNull::new_unchecked(ptr) }
}

fn map_addr<T>(x: NonNull<T>, f: impl FnOnce(NonZeroUsize) -> NonZeroUsize) -> NonNull<T> {
    with_addr(x, f(addr(x)))
}

impl<T, const N: usize> RawHazardGuard<T, N> {
    /// Get a reference to the underlying value behind the guard
    ///
    /// # Safety
    ///
    /// The Hazard this guard was derived from must still be alive and this node must be locked
    pub const unsafe fn as_ref(&self) -> &T {
        let node = self.node.as_ptr().cast_const();
        // SAFETY: The caller ensures that the Hazard is still alive
        // The hazard never removes any nodes until drop so this node
        // is still valid
        unsafe { &(*node).value }
    }

    /// Tries to the lock on the node, returning true iff node was locked
    ///
    /// # Safety
    ///
    /// The Hazard this guard was derived from must still be alive
    #[must_use]
    pub unsafe fn try_acquire(&mut self) -> bool {
        let node = map_addr(self.node, |addr| {
            let addr = addr.get() & !1;
            // SAFETY: self.node is aligned to 8 bytes
            unsafe { NonZeroUsize::new_unchecked(addr) }
        });

        // SAFETY: The caller ensures that the Hazard is still alive
        // The hazard never removes any nodes until drop so this node
        // is still valid
        let is_success = unsafe {
            (*node.as_ptr())
                .is_locked
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        };

        if is_success {
            self.node = node;
        }

        is_success
    }

    /// Releases the lock on the node
    ///
    /// # Safety
    ///
    /// The Hazard this guard was derived from must still be alive and this node must be
    /// locked
    pub unsafe fn release(&mut self) {
        // SAFETY: The caller ensures that the Hazard is still alive
        // The hazard never removes any nodes until drop so this node
        // is still valid
        // And this node must be locked, so
        unsafe {
            (*self.node.as_ptr())
                .is_locked
                .store(false, Ordering::Release);
        }

        self.node = map_addr(self.node, |x| x | 1);
    }

    /// Releases the lock on the node
    ///
    /// # Safety
    ///
    /// The Hazard this guard was derived from must still be alive
    pub unsafe fn release_if_locked(&mut self) {
        let is_locked = addr(self.node).get() & 1;
        if is_locked == 0 {
            // SAFETY: the Hazard this guard is locked
            unsafe { self.release() }
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
            // SAFETY: We are in `Drop` and all RawHazardGuard are all released before hand or
            // will not be touched after this. They are also not allowed to race with this drop
            // so we have exclusive access to all chunks
            current_chunk = unsafe { chunk.as_mut() }.next;

            // SAFETY: layout is compatible with the allocation of a chunk
            // and we have exclusive access to the chunk
            unsafe { alloc::alloc::dealloc(chunk.as_ptr().cast(), layout) }
        }
    }
}

impl<T, const N: usize> Hazard<T, N> {
    #[const_fn(cfg(not(loom)))]
    pub const fn new() -> Self {
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
        // SAFETY: Layout is guaranteed to be non-empty, because HazardNodeChunk contains a
        // pointer
        let chunk = unsafe { alloc::alloc::alloc(layout) };

        let Some(chunk) = NonNull::new(chunk) else {
            handle_alloc_error(layout)
        };

        let chunk = chunk.cast::<HazardNodeChunk<T, N>>();

        // SAFETY: chunk was just allocated, and we checked the allocation is non-null
        unsafe { core::ptr::addr_of_mut!((*chunk.as_ptr()).next).write(ptr::null_mut()) }

        // SAFETY: chunk was just allocated, and we checked the allocation is non-null
        let items = unsafe { core::ptr::addr_of_mut!((*chunk.as_ptr()).items) };
        let items: *mut CachePadded<HazardNode<T, N>> = items.cast();

        for i in 0..N {
            // SAFETY: chunk was just allocated, and we checked the allocation is non-null, we have
            // allocated N items in the array
            let node: *mut HazardNode<T, N> = unsafe { items.add(i).cast() };

            // SAFETY: the node above is valid for writes since we got it from the global allocator
            unsafe {
                node.write(HazardNode {
                    is_locked: AtomicBool::new(false),
                    value: f(),
                })
            }
        }

        // lock the first item on creation
        let first_node: *mut HazardNode<T, N> = items.cast();
        // SAFETY: There is at least one element in the list, since we checked that N is non-zero
        // in the constructor
        unsafe { (*first_node).is_locked = AtomicBool::new(true) }

        self.head
            // SAFETY: chunk was just allocated, and we checked the allocation is non-null
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |prev_head| unsafe {
                (*chunk.as_ptr()).next = prev_head;
                Some(chunk.as_ptr())
            })
            .unwrap();

        RawHazardGuard {
            // SAFETY: chunk was just allocated, and we checked the allocation is non-null
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
        // SAFETY: This iterator is constructed from a Hazard, and the lifetime ensures that
        // the Hazard is still alive, so every node in the chunk linked list is valid
        let current = unsafe { self.current?.as_ref() };
        self.current = NonNull::new(current.next);
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

    // SAFETY: the hazard is still alive
    unsafe {
        assert_eq!(*a.as_ref(), 0);
        assert_eq!(*b.as_ref(), 0);
        assert_eq!(*c.as_ref(), 0);
        assert_eq!(*d.as_ref(), 0);
        assert_eq!(*e.as_ref(), 1);
    }

    // SAFETY: the hazard is still alive
    unsafe { { b }.release_if_locked() }
    let f = hazard.get_or_insert_with(|| panic!());
    let g = hazard.get_or_insert_with(|| panic!());
    let h = hazard.get_or_insert_with(|| panic!());
    let i = hazard.get_or_insert_with(|| panic!());

    // SAFETY: the hazard is still alive
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

    // SAFETY: the hazard is still alive
    unsafe {
        { a }.release_if_locked();
        { c }.release_if_locked();
        { d }.release_if_locked();
    }

    let j = hazard.get_or_insert_with(|| panic!());
    let k = hazard.get_or_insert_with(|| panic!());
    let l = hazard.get_or_insert_with(|| panic!());

    // SAFETY: the hazard is still alive
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

#[test]
fn test_reuse_and_chunk_count() {
    let hazard = Hazard::<u8, 1>::new();

    let count_chunks = || hazard.chunks(Ordering::Relaxed).count();
    assert_eq!(count_chunks(), 0);

    let mut node = hazard.get_or_insert_with(|| 0);

    assert_eq!(count_chunks(), 1);

    // SAFETY: the hazard is still alive
    unsafe { node.release() }

    // the node above got reused
    let mut node2 = hazard.get_or_insert_with(|| panic!());

    // SAFETY: the hazard is still alive
    assert!(unsafe { !node.try_acquire() });

    // SAFETY: the hazard is still alive
    unsafe { node2.release() }

    // SAFETY: the hazard is still alive
    assert!(unsafe { node.try_acquire() });

    assert_eq!(count_chunks(), 1);

    let _node2 = hazard.get_or_insert_with(|| 1);

    assert_eq!(count_chunks(), 2);
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
