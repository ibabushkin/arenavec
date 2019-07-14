use crate::common::{self, ArenaBacking};

use std::alloc::Layout;
use std::cell::Cell;

#[derive(Debug)]
pub enum ArenaError {
    AlreadyLocked,
}

/// A statically checked arena
#[derive(Debug)]
pub struct Arena {
    /// Head of the arena space
    head: *mut u8,

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
pub struct ArenaToken<'a>{
    inner: &'a Arena,
}

#[derive(Debug)]
pub struct Slice<'a, T> {
    ptr: *mut T,
    len: usize,
    token: ArenaToken<'a>,
}

impl Arena {
    pub fn init_capacity(backing: ArenaBacking, cap: usize) -> Self {
        let head = match backing {
            ArenaBacking::MemoryMap =>
                common::create_mapping(cap),
            ArenaBacking::SystemAllocation =>
                common::create_mapping_alloc(cap),
        };
        let pos = Cell::new(0);
        let locked = Cell::new(false);

        Arena {
            head,
            pos,
            cap,
            backing,
            locked,
        }
    }

    pub fn generation_token<'a>(&'a self) -> Result<ArenaToken<'a>, ArenaError> {
        if self.locked.get() {
            Err(ArenaError::AlreadyLocked)
        } else {
            Ok(ArenaToken{ inner: self })
        }
    }
}

impl<'a> ArenaToken<'a> {
    pub fn allocate<T>(&self, count: usize) -> Slice<'a, T> {
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

        Slice {
            ptr: ret,
            len: count,
            token: self.clone(),
        }
    }
}

impl<'a> Drop for ArenaToken<'a> {
    fn drop(&mut self) {
        self.inner.pos.set(0);
        self.inner.locked.set(false);
    }
}

// TODO: Drop for Slice
