use crate::common::{self, AllocHandle, ArenaBacking, ArenaError};

use std::alloc::Layout;
use std::cell::Cell;
use std::mem;
use std::ptr::NonNull;

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
#[derive(Debug)]
pub struct ArenaToken<'a> {
    inner: &'a Arena,
}

#[derive(Debug, Clone)]
pub struct ArenaHandle<'a>(&'a ArenaToken<'a>);

pub type SliceVec<'a, T> = common::SliceVec<T, ArenaHandle<'a>>;

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

impl<'a> ArenaToken<'a> {
    pub fn weak(&'a self) -> ArenaHandle<'a> {
        ArenaHandle(self)
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

impl<'a> AllocHandle for ArenaHandle<'a> {
    fn allocate<T>(&self, count: usize) -> NonNull<T> {
        self.0.allocate(count)
    }

    fn allocate_or_extend<T>(&self, ptr: NonNull<T>, old_count: usize, count: usize) -> NonNull<T> {
        self.0.allocate_or_extend(ptr, old_count, count)
    }
}

impl<'a> Drop for ArenaToken<'a> {
    fn drop(&mut self) {
        self.inner.pos.set(0);
        self.inner.locked.set(false);
    }
}
