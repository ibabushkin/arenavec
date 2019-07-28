use std::alloc::{alloc, dealloc, Layout};
use std::cmp;
use std::fmt;
use std::marker;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};

#[derive(Debug)]
pub enum ArenaError {
    AllocationFailed,
    AlreadyLocked,
}

#[derive(Debug)]
pub enum ArenaBacking {
    MemoryMap,
    SystemAllocation,
}

pub trait AllocHandle {
    fn allocate<T>(&self, count: usize) -> NonNull<T>;
    fn allocate_or_extend<T>(&self, ptr: NonNull<T>, old_count: usize, count: usize) -> NonNull<T>;
}

pub trait ArenaSlice {
    type Item;
    type AllocHandle;

    fn get_alloc_handle(&self) -> Self::AllocHandle;
    fn ptr(&self) -> NonNull<Self::Item>;
    fn len(&self) -> usize;
    fn set_ptr(&mut self, ptr: NonNull<Self::Item>);
    fn set_len(&mut self, len: usize);
    unsafe fn new_empty(handle: Self::AllocHandle, real_len: usize) -> Self;
    fn iter<'a>(&'a self) -> SliceIter<'a, Self::Item>;
    fn iter_mut<'a>(&'a mut self) -> SliceIterMut<'a, Self::Item>;
}

/// An arena allocated, sequential, resizable vector
///
/// Since the arena does not support resizing, or freeing memory, this implementation just
/// creates new slices as necessary and leaks the previous arena allocation, trading memory
/// for speed.
pub struct SliceVec<S> {
    slice: S,
    // owo what's this
    capacity: usize,
}

/// An iterator over a sequence of arena-allocated objects
#[derive(Debug)]
pub struct SliceIter<'a, T> {
    pub(crate) ptr: *const T,
    pub(crate) end: *const T,
    pub(crate) _marker: marker::PhantomData<&'a T>,
}

/// An iterator over a mutable sequence of arena-allocated objects
#[derive(Debug)]
pub struct SliceIterMut<'a, T> {
    pub(crate) ptr: *mut T,
    pub(crate) end: *mut T,
    pub(crate) _marker: marker::PhantomData<&'a T>,
}

impl<S, T> SliceVec<S>
where
    S: ArenaSlice<Item = T>,
{
    pub fn iter<'a>(&'a self) -> SliceIter<'a, T> {
        self.slice.iter()
    }

    pub fn iter_mut<'a>(&'a mut self) -> SliceIterMut<'a, T> {
        self.slice.iter_mut()
    }
}

impl<S, T> SliceVec<S>
where
    S: ArenaSlice<Item = T>,
    <S as ArenaSlice>::AllocHandle: AllocHandle,
{
    pub fn new(inner: <S as ArenaSlice>::AllocHandle, capacity: usize) -> Self {
        SliceVec {
            slice: unsafe { S::new_empty(inner, capacity) },
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn reserve(&mut self, size: usize) {
        let ptr = self.slice.ptr();
        let handle = self.slice.get_alloc_handle();

        if self.capacity >= size {
            return;
        }

        let mut new_capacity = if self.capacity > 0 { self.capacity } else { 4 };

        while new_capacity < size {
            new_capacity *= 2;
        }

        let new_ptr: NonNull<T> = handle.allocate_or_extend(ptr, self.capacity, new_capacity);

        if ptr != new_ptr {
            unsafe {
                ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), self.slice.len());
            }

            self.slice.set_ptr(new_ptr);
        }

        self.capacity = new_capacity;
    }

    pub fn push(&mut self, elem: T) {
        if self.slice.len() == self.capacity {
            let new_capacity = if self.capacity == 0 {
                4
            } else {
                self.capacity * 2
            };

            self.reserve(new_capacity);
        }

        unsafe {
            ptr::write(self.slice.ptr().as_ptr().add(self.slice.len()), elem);
        }

        self.slice.set_len(self.slice.len() + 1);
    }

    pub fn resize(&mut self, len: usize, value: T)
    where
        T: Clone,
    {
        if self.capacity < len {
            self.reserve(len);
        }

        for i in self.slice.len()..len.saturating_sub(1) {
            unsafe { ptr::write(self.slice.ptr().as_ptr().add(i), value.clone()) }
        }

        if len > self.slice.len() {
            unsafe {
                ptr::write(self.slice.ptr().as_ptr().add(len - 1), value);
            }
        }

        self.slice.set_len(len);
    }

    pub fn clear(&mut self) {
        self.slice.set_len(0);
    }
}

impl<S, T> Clone for SliceVec<S>
where
    S: ArenaSlice<Item = T>,
    <S as ArenaSlice>::AllocHandle: AllocHandle,
    T: Clone,
{
    fn clone(&self) -> Self {
        let mut vec: SliceVec<S> = SliceVec::new(self.slice.get_alloc_handle(), self.capacity);

        for i in 0..self.slice.len() {
            unsafe {
                ptr::write(
                    vec.slice.ptr().as_ptr().add(i),
                    (*self.slice.ptr().as_ptr().add(i)).clone(),
                );
            }
        }

        vec.slice.set_len(self.slice.len());

        vec
    }
}

impl<S: fmt::Debug> fmt::Debug for SliceVec<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.slice.fmt(fmt)
    }
}

impl<S: Deref> Deref for SliceVec<S> {
    type Target = <S as Deref>::Target;

    fn deref(&self) -> &<S as Deref>::Target {
        self.slice.deref()
    }
}

impl<S, T> DerefMut for SliceVec<S>
where
    S: DerefMut,
    S: ArenaSlice<Item = T>,
{
    fn deref_mut(&mut self) -> &mut <S as std::ops::Deref>::Target {
        self.slice.deref_mut()
    }
}

impl<S> Eq for SliceVec<S>
where
    S: Deref,
    <S as Deref>::Target: Eq,
{
}

impl<S> PartialEq for SliceVec<S>
where
    S: Deref,
    <S as Deref>::Target: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}

impl<S> PartialOrd for SliceVec<S>
where
    S: Deref,
    <S as Deref>::Target: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

impl<'a, S, T: 'a> IntoIterator for &'a SliceVec<S>
where
    S: ArenaSlice<Item = T>,
{
    type Item = &'a T;
    type IntoIter = SliceIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, S, T: 'a> IntoIterator for &'a mut SliceVec<S>
where
    S: ArenaSlice<Item = T>,
{
    type Item = &'a mut T;
    type IntoIter = SliceIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<'a, T> Iterator for SliceIter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            unsafe {
                // FIXME:
                // we do not support ZSTs right now, the stdlib does some dancing
                // for this which we can safely avoid for now
                let old = self.ptr;
                self.ptr = self.ptr.offset(1);
                Some(&*old)
            }
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // let len = unsafe { self.end.offset_from(self.ptr) } as usize;
        let ptr = self.ptr;
        let diff = (self.end as usize).wrapping_sub(ptr as usize);
        let len = diff / mem::size_of::<T>();

        (len, Some(len))
    }
}

impl<'a, T> DoubleEndedIterator for SliceIter<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            unsafe {
                // FIXME:
                // we do not support ZSTs right now, the stdlib does some dancing
                // for this which we can safely avoid for now
                let old = self.end;
                self.end = self.end.offset(-1);
                Some(&*old)
            }
        }
    }
}

impl<'a, T> ExactSizeIterator for SliceIter<'a, T> {}

impl<'a, T> Iterator for SliceIterMut<'a, T> {
    type Item = &'a mut T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            unsafe {
                // FIXME:
                // we do not support ZSTs right now, the stdlib does some dancing
                // for this which we can safely avoid for now
                let old = self.ptr;
                self.ptr = self.ptr.offset(1);
                Some(&mut *old)
            }
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // let len = unsafe { self.end.offset_from(self.ptr) } as usize;
        let ptr = self.ptr;
        let diff = (self.end as usize).wrapping_sub(ptr as usize);
        let len = diff / mem::size_of::<T>();

        (len, Some(len))
    }
}

impl<'a, T> DoubleEndedIterator for SliceIterMut<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.ptr == self.end {
            None
        } else {
            unsafe {
                // FIXME:
                // we do not support ZSTs right now, the stdlib does some dancing
                // for this which we can safely avoid for now
                let old = self.end;
                self.end = self.end.offset(-1);
                Some(&mut *old)
            }
        }
    }
}

impl<'a, T> ExactSizeIterator for SliceIterMut<'a, T> {}

#[cfg(unix)]
pub(crate) fn get_page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

#[cfg(windows)]
pub(crate) fn get_page_size() -> usize {
    use std::mem;
    use winapi::um::sysinfoapi::GetSystemInfo;

    unsafe {
        let mut info = mem::zeroed();
        GetSystemInfo(&mut info);

        info.dwPageSize as usize
    }
}

#[cfg(unix)]
pub(crate) fn create_mapping(capacity: usize) -> *mut u8 {
    let ptr = unsafe {
        libc::mmap(
            ptr::null_mut(),
            capacity,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANON | libc::MAP_PRIVATE,
            -1,
            0,
        )
    };

    ptr as *mut u8
}

#[cfg(windows)]
pub(crate) fn create_mapping(capacity: usize) -> *mut u8 {
    use std::ptr;
    use winapi::shared::basetsd::SIZE_T;
    use winapi::shared::minwindef::LPVOID;
    use winapi::um::memoryapi::VirtualAlloc;
    use winapi::um::winnt::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};

    let lpAddress: LPVOID = ptr::null_mut();
    let page_size = get_page_size();
    let len = if capacity % page_size == 0 {
        capacity
    } else {
        capacity + page_size - (capacity % page_size)
    };
    let flAllocationType = MEM_COMMIT | MEM_RESERVE;
    let flProtect = PAGE_READWRITE;

    let r = unsafe { VirtualAlloc(lpAddress, len as SIZE_T, flAllocationType, flProtect) };

    r as *mut u8
}

pub(crate) fn create_mapping_alloc(capacity: usize) -> *mut u8 {
    unsafe { alloc(Layout::from_size_align_unchecked(capacity, get_page_size())) }
}

#[cfg(unix)]
pub(crate) fn destroy_mapping(base: NonNull<u8>, capacity: usize) {
    let res = unsafe { libc::munmap(base.as_ptr() as *mut libc::c_void, capacity) };

    // TODO: Do something on error
    debug_assert_eq!(res, 0);
}

#[cfg(windows)]
pub(crate) fn destroy_mapping(base: NonNull<u8>, capacity: usize) {
    use winapi::shared::minwindef::LPVOID;
    use winapi::um::memoryapi::VirtualFree;
    use winapi::um::winnt::MEM_RELEASE;

    let res = unsafe { VirtualFree(base.as_ptr() as LPVOID, 0, MEM_RELEASE) };

    // TODO: Do something on error
    debug_assert_ne!(res, 0);
}

pub(crate) fn destroy_mapping_alloc(base: NonNull<u8>, capacity: usize) {
    unsafe {
        let layout = Layout::from_size_align_unchecked(capacity, get_page_size());
        dealloc(base.as_ptr(), layout);
    }
}
