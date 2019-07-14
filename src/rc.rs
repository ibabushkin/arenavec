use crate::common::{self, AllocHandle, ArenaBacking, ArenaSlice, SliceIter, SliceIterMut};

use std::alloc::Layout;
use std::cell::Cell;
use std::cmp;
use std::fmt;
// use std::iter::FromIterator;
use std::marker;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::rc::Rc;
use std::slice;

/* #[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer}; */

/// The owning object of the arena.
///
/// Can be stored in a thread-local variable to be accessible everywhere in the owning thread.
/// This is the only instance that can be used to allocate and clear the arena. All other
/// objects referring to the arena merely keep it alive, and are present to avoid arena
/// clearing while they are live. Also keeps track of how the backing memory has been acquired.
#[derive(Debug)]
pub struct Arena(InnerRef, ArenaBacking);

/// A non-owning object of the arena.
///
/// A reference to the arena that allows its holder to allocate memory from the arena. While
/// it is live, the arena cannot be cleared (as it is associated with an arena-allocated
/// object).
#[derive(Clone, Debug)]
pub struct InnerRef {
    inner: Rc<Inner>,
}

/// An arena's guts
#[derive(Debug)]
struct Inner {
    /// Head of the arena space
    head: *mut u8,

    /// Offset into the last region
    pos: Cell<usize>,

    /// Total capacity of the arena
    cap: usize,
}

/// An arena allocated, fixed-size sequence of objects
pub struct Slice<T> {
    ptr: *mut T,
    len: usize,
    _inner: InnerRef,
}

pub type SliceVec<T> = common::SliceVec<Slice<T>>;

impl Arena {
    /// Create an `Arena` with specified capacity.
    ///
    /// Capacity must be a power of 2. The capacity cannot be grown after the fact.
    pub fn init_capacity(backing: ArenaBacking, cap: usize) -> Self {
        let head = match backing {
            ArenaBacking::MemoryMap => common::create_mapping(cap),
            ArenaBacking::SystemAllocation => common::create_mapping_alloc(cap),
        };
        let pos = Cell::new(0);

        Arena(
            InnerRef {
                inner: Rc::new(Inner { head, pos, cap }),
            },
            backing,
        )
    }

    /// Create another reference to the arena's guts.
    pub fn inner(&self) -> InnerRef {
        self.0.clone()
    }

    /// Clear the arena.
    ///
    /// This only requires an immutable reference, as it (a) perfors a check that
    /// no arena-allocated object is still alive (weak reason), and because all mutable
    /// state is neatly contained in a `Cell` (slightly stronger reason).
    pub fn clear(&self) {
        assert!(1 == Rc::strong_count(&self.inner));
        self.inner.pos.set(0);
    }
}

impl Deref for Arena {
    type Target = InnerRef;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        match self.1 {
            ArenaBacking::MemoryMap => {
                common::destroy_mapping(self.inner.head, self.inner.cap);
            }
            ArenaBacking::SystemAllocation => {
                common::destroy_mapping_alloc(self.inner.head, self.inner.cap);
            }
        }
    }
}

impl AllocHandle for InnerRef {
    fn allocate<T>(&self, count: usize) -> *mut T {
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

        let ret = unsafe { self.inner.head.add(pos + skip) as *mut T };

        debug_assert!((ret as usize) >= self.inner.head as usize);
        debug_assert!((ret as usize) < (self.inner.head as usize + self.inner.cap));

        ret
    }

    fn allocate_or_extend<T>(&self, ptr: *mut T, old_count: usize, count: usize) -> *mut T {
        if ptr.is_null() {
            return self.allocate(count);
        }

        let pos = self.inner.pos.get();
        let next = unsafe { self.inner.head.add(pos) };
        let end = unsafe { ptr.add(old_count) };
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

impl<T> ArenaSlice for Slice<T> {
    type Item = T;
    type AllocHandle = InnerRef;

    fn get_alloc_handle(&self) -> Self::AllocHandle {
        self._inner.clone()
    }

    fn ptr(&self) -> *mut Self::Item {
        self.ptr
    }

    fn len(&self) -> usize {
        self.len
    }

    fn set_ptr(&mut self, ptr: *mut Self::Item) {
        self.ptr = ptr;
    }

    fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    /* unsafe fn from_raw(inner: Self::AllocHandle, ptr: *mut T, len: usize) -> Self {
        Slice {
            ptr,
            len,
            _inner: inner,
        }
    }

    unsafe fn into_raw(self) -> (Self::AllocHandle, *mut T, usize) {
        let Self{ ptr, len, .. } = self;
        let inner = mem::transmute_copy(&self._inner);
        mem::forget(self);

        (inner, ptr, len)
    } */

    unsafe fn new_empty(inner: Self::AllocHandle, real_len: usize) -> Self {
        let ptr: *mut T = if real_len == 0 {
            ptr::NonNull::dangling().as_ptr()
        } else {
            inner.allocate(real_len)
        };

        Slice {
            ptr,
            len: 0,
            _inner: inner,
        }
    }

    fn iter<'a>(&'a self) -> SliceIter<'a, T> {
        unsafe {
            // no ZST support
            let ptr = self.ptr;
            let end = self.ptr.add(self.len);

            SliceIter {
                ptr,
                end,
                _marker: marker::PhantomData,
            }
        }
    }

    fn iter_mut<'a>(&'a mut self) -> SliceIterMut<'a, T> {
        unsafe {
            // no ZST support
            let ptr = self.ptr;
            let end = self.ptr.add(self.len);

            SliceIterMut {
                ptr,
                end,
                _marker: marker::PhantomData,
            }
        }
    }
}

impl<T> Slice<T> {
    pub fn new(inner: InnerRef, len: usize) -> Self
    where
        T: Default,
    {
        let mut res = unsafe { Self::new_empty(inner, len) };
        res.len = len;

        for i in 0..len {
            unsafe {
                ptr::write(res.ptr.add(i), T::default());
            }
        }

        res
    }
}

impl<T: Clone> Clone for Slice<T> {
    fn clone(&self) -> Self {
        let ptr: *mut T = self._inner.allocate(self.len);

        for i in 0..self.len {
            unsafe {
                ptr::write(ptr.add(i), (*self.ptr.add(i)).clone());
            }
        }

        Slice {
            ptr,
            len: self.len,
            _inner: self._inner.clone(),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Slice<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(fmt)
    }
}

impl<T> Deref for Slice<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T> DerefMut for Slice<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl<T: Eq> Eq for Slice<T> {}

impl<T: PartialEq> PartialEq for Slice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}

impl<T: PartialOrd> PartialOrd for Slice<T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

impl<T> Drop for Slice<T> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(&mut self[..]);
        }
    }
}

/* #[cfg(feature = "serde")]
impl<T> Serialize for Slice<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.iter())
    }
}

#[cfg(feature = "serde")]
impl<'de, T> Deserialize<'de> for Slice<T>
where
    T: Deserialize<'de>,
{
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut res: Vec<T> = Deserialize::deserialize(deserializer)?;
        let mut slice = Slice::new(res.len());

        unsafe {
            let ptr = res.as_mut_ptr();
            ptr::copy_nonoverlapping(slice.ptr, ptr, slice.len);
            dealloc(ptr);
        }

        mem::forget(res);

        slice
    }
} */

impl<'a, T> IntoIterator for &'a Slice<T> {
    type Item = &'a T;
    type IntoIter = SliceIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Slice<T> {
    type Item = &'a mut T;
    type IntoIter = SliceIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/*#[cfg(feature = "serde")]
impl<T> Serialize for SliceVec<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.slice.serialize(serializer)
    }
}

impl<T> FromIterator<T> for SliceVec<T> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item=T>
    {
        let iter = iter.into_iter();
        let (min, max) = iter.size_hint();
        let cap = if let Some(m) = max { m } else { min };

        let mut res = SliceVec::new(cap);

        for e in iter {
            res.push(e);
        }

        res
    }
}*/
