use crate::common::{
    self, AllocHandle, ArenaBacking, ArenaError, ArenaSlice, SliceIter, SliceIterMut,
};

use std::alloc::Layout;
use std::cell::Cell;
use std::cmp;
use std::fmt;
use std::marker;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use std::slice;

/// A statically checked arena
#[derive(Debug)]
pub struct Arena {
    /// Head of the arena space
    head: NonNull<u8>,

    /// Offset into the last region
    pos: Cell<usize>,

    /// Total capacity of the arena
    cap: usize,

    /// The type of backing storage used in the arena
    backing: ArenaBacking,

    /// Whether an exclusive allocation token has been handed out
    locked: Cell<bool>,
}

/// A proxy for an arena that actually allows allocation.
///
/// The intention is to ensure exclusive allocation access and to tag allocated objects with
/// the lifetime.
#[derive(Debug, Clone)]
pub struct ArenaToken<'a> {
    inner: &'a Arena,
}

pub struct Slice<'a, T> {
    ptr: NonNull<T>,
    len: usize,
    token: &'a ArenaToken<'a>,
}

pub type SliceVec<'a, T> = common::SliceVec<Slice<'a, T>>;

impl Arena {
    pub fn init_capacity(backing: ArenaBacking, cap: usize) -> Result<Self, ArenaError> {
        let head = NonNull::new(match backing {
            ArenaBacking::MemoryMap => common::create_mapping(cap),
            ArenaBacking::SystemAllocation => common::create_mapping_alloc(cap),
        })
        .ok_or(ArenaError::AllocationFailed)?;
        let pos = Cell::new(0);
        let locked = Cell::new(false);

        Ok(Arena {
            head,
            pos,
            cap,
            backing,
            locked,
        })
    }

    pub fn generation_token<'a>(&'a self) -> Result<ArenaToken<'a>, ArenaError> {
        if self.locked.get() {
            Err(ArenaError::AlreadyLocked)
        } else {
            self.locked.set(true);
            Ok(ArenaToken { inner: self })
        }
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        match self.backing {
            ArenaBacking::MemoryMap => {
                common::destroy_mapping(self.head, self.cap);
            }
            ArenaBacking::SystemAllocation => {
                common::destroy_mapping_alloc(self.head, self.cap);
            }
        }
    }
}

impl<'a> AllocHandle for ArenaToken<'a> {
    fn allocate<T>(&self, count: usize) -> NonNull<T> {
        let layout = Layout::new::<T>();
        let mask = layout.align() - 1;
        let pos = self.inner.pos.get();

        debug_assert!(layout.align() >= (pos & mask));

        // let align = Ord::max(layout.align(), 64);
        let mut skip = 64 - (pos & mask);

        if skip == layout.align() {
            skip = 0;
        }

        let additional = skip + layout.size() * count;

        assert!(
            pos + additional <= self.inner.cap,
            "arena overflow: {} > {}",
            pos + additional,
            self.inner.cap
        );

        self.inner.pos.set(pos + additional);

        let ret = unsafe { self.inner.head.as_ptr().add(pos + skip) as *mut T };

        assert!((ret as usize) >= self.inner.head.as_ptr() as usize);
        assert!((ret as usize) < (self.inner.head.as_ptr() as usize + self.inner.cap));

        unsafe { NonNull::new_unchecked(ret) }
    }

    fn allocate_or_extend<T>(&self, ptr: NonNull<T>, old_count: usize, count: usize) -> NonNull<T> {
        let pos = self.inner.pos.get();
        let next = unsafe { self.inner.head.as_ptr().add(pos) };
        let end = unsafe { ptr.as_ptr().add(old_count) };
        if next == end as *mut u8 {
            self.inner
                .pos
                .set(pos + (count - old_count) * mem::size_of::<T>());

            ptr
        } else {
            self.allocate(count)
        }
    }
}

impl<'a> AllocHandle for &'a ArenaToken<'a> {
    fn allocate<T>(&self, count: usize) -> NonNull<T> {
        (*self).allocate(count)
    }

    fn allocate_or_extend<T>(&self, ptr: NonNull<T>, old_count: usize, count: usize) -> NonNull<T> {
        (*self).allocate_or_extend(ptr, old_count, count)
    }
}

impl<'a> Drop for ArenaToken<'a> {
    fn drop(&mut self) {
        self.inner.pos.set(0);
        self.inner.locked.set(false);
    }
}

impl<'a, T> ArenaSlice for Slice<'a, T> {
    type Item = T;
    type AllocHandle = &'a ArenaToken<'a>;

    fn get_alloc_handle(&self) -> Self::AllocHandle {
        self.token
    }

    fn ptr(&self) -> NonNull<Self::Item> {
        self.ptr
    }

    fn len(&self) -> usize {
        self.len
    }

    fn set_ptr(&mut self, ptr: NonNull<Self::Item>) {
        self.ptr = ptr;
    }

    fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    unsafe fn new_empty(token: Self::AllocHandle, real_len: usize) -> Self {
        let ptr: NonNull<T> = if real_len == 0 {
            NonNull::dangling()
        } else {
            token.allocate(real_len)
        };

        Slice { ptr, len: 0, token }
    }

    fn iter<'b>(&'b self) -> SliceIter<'b, T> {
        unsafe {
            // no ZST support
            let ptr = self.ptr.as_ptr();
            let end = ptr.add(self.len);

            SliceIter {
                ptr,
                end,
                _marker: marker::PhantomData,
            }
        }
    }

    fn iter_mut<'b>(&'b mut self) -> SliceIterMut<'b, T> {
        unsafe {
            // no ZST support
            let ptr = self.ptr.as_ptr();
            let end = ptr.add(self.len);

            SliceIterMut {
                ptr,
                end,
                _marker: marker::PhantomData,
            }
        }
    }
}

impl<'a, T: Clone> Clone for Slice<'a, T> {
    fn clone(&self) -> Self {
        let ptr: NonNull<T> = self.token.allocate(self.len);

        for i in 0..self.len {
            unsafe {
                ptr::write(ptr.as_ptr().add(i), (*self.ptr.as_ptr().add(i)).clone());
            }
        }

        Slice {
            ptr,
            len: self.len,
            token: self.token,
        }
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for Slice<'a, T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(fmt)
    }
}

impl<'a, T> Deref for Slice<'a, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<'a, T> DerefMut for Slice<'a, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<'a, T: Eq> Eq for Slice<'a, T> {}

impl<'a, T: PartialEq> PartialEq for Slice<'a, T> {
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}

impl<'a, T: PartialOrd> PartialOrd for Slice<'a, T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

impl<'a, T> Drop for Slice<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(&mut self[..]);
        }
    }
}

impl<'a, T> IntoIterator for &'a Slice<'a, T> {
    type Item = &'a T;
    type IntoIter = SliceIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Slice<'a, T> {
    type Item = &'a mut T;
    type IntoIter = SliceIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
