use crate::common::{self, AllocHandle, ArenaBacking, ArenaError};

use std::alloc::Layout;
use std::cell::Cell;
use std::mem;
use std::ops::Deref;
use std::ptr::NonNull;
use std::rc::Rc;

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
    head: NonNull<u8>,

    /// Offset into the last region
    pos: Cell<usize>,

    /// Total capacity of the arena
    cap: usize,
}

/// An arena allocated, fixed-size sequence of objects
pub type Slice<T> = common::Slice<T, InnerRef>;

pub type SliceVec<T> = common::SliceVec<T, InnerRef>;

impl Arena {
    /// Create an `Arena` with specified capacity.
    ///
    /// Capacity must be a power of 2. The capacity cannot be grown after the fact.
    pub fn init_capacity(backing: ArenaBacking, cap: usize) -> Result<Self, ArenaError> {
        let head = NonNull::new(match backing {
            ArenaBacking::MemoryMap => common::create_mapping(cap),
            ArenaBacking::SystemAllocation => common::create_mapping_alloc(cap),
        })
        .ok_or(ArenaError::AllocationFailed)?;
        let pos = Cell::new(0);

        Ok(Arena(
            InnerRef {
                inner: Rc::new(Inner { head, pos, cap }),
            },
            backing,
        ))
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
