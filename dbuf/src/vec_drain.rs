use alloc::vec::Vec;
use core::{marker::PhantomData, ops::RangeTo};

pub struct Drain<'a, T> {
    ptr: *mut T,
    end: *mut T,
    old_len: usize,
    vec: *mut Vec<T>,
    lt: PhantomData<&'a mut Vec<T>>,
}

pub fn drain_unti<T>(vec: &mut Vec<T>, n: RangeTo<usize>) -> Drain<'_, T> {
    const { assert!(core::mem::size_of::<T>() > 0) }
    let _ = &vec[n];
    let old_len = vec.len();
    // SAFETY: the index above validates that the range is in bounds
    unsafe { vec.set_len(n.end) };

    let range = vec.as_mut_ptr_range();

    Drain {
        ptr: range.start,
        end: range.end,
        old_len,
        vec,
        lt: PhantomData,
    }
}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        // SAFETY: this vector pointer came from a vector reference
        // whose lifetime is tied to the drain. So it must still be valid
        let vec = unsafe { &mut *self.vec };
        let range_start = vec.as_mut_ptr();
        // SAFETY: adding the original length of the vector won't go out of bounds
        let range_end = unsafe { range_start.add(self.old_len) };
        // SAFETY: self.ptr is in the same allocation as the vector
        let remaining = unsafe { range_end.offset_from(self.ptr) } as usize;
        // SAFETY: self.ptr is valid for writes, and range.start is valid for reads for remaining
        // elements
        unsafe { self.ptr.copy_to(range_start, remaining) };
        // SAFETY: all items from 0..remaining are initialized
        unsafe { vec.set_len(remaining) }
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            // SAFETY: self.ptr hasn't yet reached the end
            // so we can increment it and read from it since it
            // must point to inside the vector
            unsafe {
                let value = self.ptr.read();
                self.ptr = self.ptr.add(1);
                Some(value)
            }
        }
    }
}
