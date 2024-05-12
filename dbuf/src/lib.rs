#![no_std]
#![forbid(
    unsafe_op_in_unsafe_fn,
    clippy::missing_safety_doc,
    clippy::undocumented_unsafe_blocks,
    clippy::suspicious_doc_comments,
    clippy::missing_const_for_fn,
    clippy::suspicious,
    clippy::branches_sharing_code,
    clippy::bad_bit_mask,
    clippy::std_instead_of_core,
    clippy::alloc_instead_of_core,
    clippy::std_instead_of_alloc
)]
#![cfg_attr(
    not(test),
    forbid(
        clippy::print_stderr,
        clippy::print_stdout,
        clippy::todo,
        clippy::dbg_macro
    )
)]
#![deny(clippy::perf, clippy::arithmetic_side_effects, unused_unsafe)]
#![allow(clippy::declare_interior_mutable_const)]

//! # dbuf
//!
//! This crate provides generic async-aware implementations for pointers to double buffered
//! values. This is a rather low level crate that exposes a lot of details to the
//! users. It is meant to ease writing more high-level data structures. But it can be
//! used stand-alone, if need be.
//!
//! The main types are [`raw::Writer`] and [`raw::Reader`]. This type is parameterized
//! by a pointer type, a strategy, and a buffer type of your choosing.
//!
//! This type also provides a wrapper around [`raw::Writer`] which allows you to
//! start swaps early using [`delay::DelayWriter`]. This is useful for batched writes, where you can start a
//! swap right after a batch is complete and complete the swap much later
//! when you write the next batch.
//!
//! ## Supported Pointer Types
//!
//! The types here are listed as `shared pointer`/`unique pointer`
//!
//! You must use the `unique pointer` to construct the [`raw::Writer`]
//!
//! See [Worked Example](#worked-example) an example of how this works in practice.
//!
//! By default this crate provides implementations for these pointer types
//!
//! * References ([`&`] [`raw::DoubleBufferData`]) / [`&mut`]
//!
//! These make the [`raw::Reader`] cheap to copy (and allows the [`raw::Reader`] to implement
//! [`Copy`])
//!
//! * [`std::rc::Rc`]/[`rc_box::RcBox`]
//!
//! Cheap to copy [`raw::Reader`], not thread-safe
//!
//! * [`std::sync::Arc`]/[`rc_box::ArcBox`]
//!
//! It allows the data to be destroyed as soon as the writer is dropped and
//! all outstanding reads are complete. (does not require all readers to be dropped).
//!
//! Expensive to copy [`raw::Reader`] in a tight loop, since it requires an atomic increment/decrement.
//!
//! But it allows the data to be destroyed as soon as the writer is dropped and
//! all outstanding reads are complete. (does not require all readers to be dropped).
//!
//! * [`triomphe::OffsetArc`]/[`triomphe::UniqueArc`]
//!
//! Cheap to copy [`raw::Reader`], but all readers must be dropped before the buffers are freed.
//!
//! ### Custom Pointer types
//!
//! But you can implement the triple trait combo of [`interface::IntoDoubleBufferWriterPointer`],
//! [`interface::DoubleBufferWriterPointer`], and [`interface::DoubleBufferReaderPointer`]
//! if this selection isn't to your liking.
//!
//! ## Strategy types
//!
//! This crate provides a few default strategy types
//!
//! * [`strategy::simple::SimpleStrategy`] - A non-thread-safe strategy which just keeps two
//! counters for how many readers are in each buffer, it checks these counts whenever you try to
//! swap the buffers, and errors if there are any readers in the other buffer.
//! * [`strategy::simple_async::SimpleAsyncStrategy`] - A non-thread-safe strategy which just keeps two
//! counters for how many readers are in each buffer. It waits until there are no more readers in
//! the other buffer before swapping.
//! * [`strategy::atomic::AtomicStrategy`] - A thread-safe strategy which just keeps two
//! counters for how many readers are in each buffer, it checks these counts whenever you try to
//! swap the buffers, and errors if there are any readers in the other buffer.
//! * [`strategy::flashmap::FlashStrategy`] - A thread-safe strategy that is based off of the
//! [`flashmap`](https://docs.rs/flashmap) crate. see module level docs for details.
//!
//! ## Worked Example
//!
//! You can use these together like so.
//!
//! ```rust
//! use dbuf::raw::{Writer, Reader, DoubleBufferData};
//! use dbuf::strategy::simple::SimpleStrategy;
//!
//! let front = 10;
//! let back = 300;
//!
//! let mut data = DoubleBufferData::new(front, back, SimpleStrategy::new());
//! let mut writer: Writer<&DoubleBufferData<i32, SimpleStrategy>> = Writer::new(&mut data);
//!                // ^^^ note how this is a shared reference
//!                // this is because we user &mut's exclusivity to prove that
//!                // the writer has exclusive access to the buffers, but
//!                // after that, it needs to be downgraded so the buffers can be
//!                // shared between the writer and readers
//!
//! // The reader must be mutable, because raw::Reader::read takes a &mut reference
//! // to prove that the read isn't re-entrant. This allows for a more optimized implementation
//! // that doesn't need to count how many times a reader has started reading.
//! let mut reader = writer.reader();
//!
//! // NOTE: that since we used references, read can't panic
//! // but if we used the std lib's Rc/Arc this will panic if the
//! // writer is dropped. You can use `raw::Reader::try_read` to
//! // check if the writer is dropped
//! let mut guard = reader.read();
//!
//! // the front buffer is shown first
//! assert_eq!(*guard, 10);
//! assert_eq!(*writer.get(), 300);
//!
//! // there is an active reader, so we can't swap yet
//! assert!(writer.try_swap().is_err());
//! assert_eq!(*guard, 10);
//! assert_eq!(*writer.get_mut(), 300);
//!
//! // end the active read, normally in your code this will happen
//! // at the end of the scope automatically.
//! drop(guard);
//!
//! // since the reader is dropped above, it is now safe to swap the buffers
//! assert!(writer.try_swap().is_ok());
//! assert_eq!(*writer.get_mut(), 10);
//! ```
//!
//! Let's see how this example changes with a different implementation.
//!
//! ```rust
//! use dbuf::raw::{Writer, Reader, DoubleBufferData};
//! use dbuf::strategy::flashmap::FlashStrategy;
//!
//! use rc_box::ArcBox;
//! use std::sync::Arc;
//!
//! let front = 10;
//! let back = 300;
//!
//! let mut data = DoubleBufferData::new(front, back, FlashStrategy::new_blocking());
//! let mut writer: Writer<Arc<DoubleBufferData<i32, FlashStrategy<_>>>> = Writer::new(ArcBox::new(data));
//!                // ^^^ note how this is a arc now, this is for a similar reason above
//!                // use use `ArcBox`'s uniqueness guarantee to ensure
//!                // the writer has exclusive access to the buffers, but
//!                // after that, it needs to be downgraded so the buffers can be
//!                // shared between the writer and readers
//!
//! let mut reader = writer.reader();
//!
//! // NOTE: you should handle the errors properly, instead of using unwrap
//! // for this example, we know that the writer is still alive, so the unwrap
//! // is justified
//! let mut guard = reader.try_read().unwrap();
//!
//! // the front buffer is shown first
//! assert_eq!(*guard, 10);
//! assert_eq!(*writer.get(), 300);
//!
//! // this would block, until there are no more readers reading
//! // writer.try_swap().unwrap();
//!
//! // end the active read, normally in your code this will happen
//! // at the end of the scope automatically.
//! drop(guard);
//!
//! // since the reader is dropped above, it is now safe to swap the buffers
//! assert!(writer.try_swap().is_ok());
//! assert_eq!(*writer.get_mut(), 10);
//! ```

#[cfg(feature = "alloc")]
extern crate alloc;

#[macro_use]
#[cfg(feature = "std")]
extern crate std;

pub mod interface;

mod ext;
pub mod strategy;

pub mod delay;
#[cfg(feature = "alloc")]
pub mod op;
pub mod raw;

#[doc(hidden)]
pub mod macros;
