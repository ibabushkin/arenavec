use crate::common::{self, ArenaBacking};

use std::cell::Cell;
use std::marker;

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
#[derive(Debug)]
pub struct ArenaToken<'a>(&'a Arena);

#[derive(Debug)]
pub struct Slice<'a, T> {
    ptr: *mut T,
    len: usize,
    _phantom: marker::PhantomData<&'a T>,
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
            Ok(ArenaToken(self))
        }
    }
}

// TODO: impl Drop for ArenaToken
