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
        let vec = unsafe { &mut *self.vec };
        let range_start = vec.as_mut_ptr();
        let range_end = unsafe { range_start.add(self.old_len) };
        let remaining = unsafe { range_end.offset_from(self.ptr) } as usize;
        unsafe { self.ptr.copy_to(range_start, remaining) };
        unsafe { vec.set_len(remaining) }
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            unsafe {
                let value = self.ptr.read();
                self.ptr = self.ptr.add(1);
                Some(value)
            }
        }
    }
}
