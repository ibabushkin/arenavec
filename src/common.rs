use std::alloc::{alloc, dealloc, Layout};
use std::cmp;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use std::slice;

/// An error type representing errors possible during arena creation or other arena operations.
#[derive(Debug)]
pub enum ArenaError {
    /// The backing storage for the arena could not be allocated.
    AllocationFailed,
    /// If an arena is locked by some token type, it refuses locking when already locked.
    AlreadyLocked,
}

/// The kind of backing requested for an arena.
#[derive(Debug)]
pub enum ArenaBacking {
    /// Create a virtual memory mapping via `mmap()` or `VirtualAlloc()`.
    MemoryMap,
    /// Ask the system allocator for the memory.
    SystemAllocation,
}

/// Every arena-allocated object has some form of handle to the arena containing it.
///
/// Depending on the type of arena, the actual functionality of the handle can be different,
/// but it at least allows for allocation and object resizing.
///
/// To be useful, handles need to implement `Clone`.
pub trait AllocHandle {
    /// Allocate memory from the arena.
    ///
    /// Allocate `count` objects of type `T` from the arena, and panic if this is not possible.
    fn allocate<T>(&self, count: usize) -> NonNull<T>;
    /// Reallocate memory in the arena.
    ///
    /// Resize the object sequence pointed to by `ptr` of `old_count` elements of type `T` to
    /// `count` objects, and copy the sequence if no resizing in place is possible.
    ///
    /// `ptr` must point into the arena.
    fn allocate_or_extend<T>(&self, ptr: NonNull<T>, old_count: usize, count: usize) -> NonNull<T>;
}

/// An arena allocated, fixed-size sequence of objects.
pub struct Slice<T, H> {
    ptr: NonNull<T>,
    len: usize,
    handle: H,
}

/// An arena allocated, sequential, resizable vector
///
/// Since the arena does not support resizing, or freeing memory, this implementation just
/// creates new slices as necessary and leaks the previous arena allocation, trading memory
/// for speed.
pub struct SliceVec<T, H> {
    slice: Slice<T, H>,
    // owo what's this
    capacity: usize,
}

impl<T, H: AllocHandle> Slice<T, H> {
    /// Create a new slice of default-initialized objects using the provided handle.
    pub fn new(handle: H, len: usize) -> Self
    where
        T: Default,
    {
        let mut res = unsafe { Self::new_empty(handle, len) };
        res.len = len;

        for i in 0..len {
            unsafe {
                ptr::write(res.ptr.as_ptr().add(i), T::default());
            }
        }

        res
    }

    /// Create a new slice of size `real_len`, but initialize length to `0`.
    unsafe fn new_empty(handle: H, real_len: usize) -> Self {
        let ptr: NonNull<T> = if real_len == 0 {
            NonNull::dangling()
        } else {
            handle.allocate(real_len)
        };

        Slice {
            ptr,
            len: 0,
            handle,
        }
    }
}

impl<T: Clone, H: AllocHandle + Clone> Clone for Slice<T, H> {
    fn clone(&self) -> Self {
        let ptr: NonNull<T> = self.handle.allocate(self.len);

        for i in 0..self.len {
            unsafe {
                ptr::write(ptr.as_ptr().add(i), (*self.ptr.as_ptr().add(i)).clone());
            }
        }

        Slice {
            ptr,
            len: self.len,
            handle: self.handle.clone(),
        }
    }
}

impl<T: fmt::Debug, H> fmt::Debug for Slice<T, H> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(fmt)
    }
}

impl<T, H> Deref for Slice<T, H> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T, H> DerefMut for Slice<T, H> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T: Eq, H> Eq for Slice<T, H> {}

impl<T: PartialEq, H> PartialEq for Slice<T, H> {
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}

impl<T: PartialOrd, H> PartialOrd for Slice<T, H> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

impl<'a, T, H> IntoIterator for &'a Slice<T, H> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.deref().iter()
    }
}

impl<'a, T, H> IntoIterator for &'a mut Slice<T, H> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.deref_mut().iter_mut()
    }
}

impl<T, H> Drop for Slice<T, H> {
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(&mut self[..]);
        }
    }
}

impl<T, H> SliceVec<T, H> {
    /// Create an immutable iterator over the elements of the vector.
    pub fn iter<'a>(&'a self) -> slice::Iter<'a, T> {
        self.slice.iter()
    }

    /// Create an mutable iterator over the elements of the vector.
    pub fn iter_mut<'a>(&'a mut self) -> slice::IterMut<'a, T> {
        self.slice.iter_mut()
    }
}

impl<T, H: AllocHandle > SliceVec<T, H> {
    /// Create a new vector of given capacity using the given handle.
    pub fn new(handle: H, capacity: usize) -> Self {
        SliceVec {
            slice: unsafe { Slice::new_empty(handle, capacity) },
            capacity,
        }
    }

    /// Return the current capacity of the vector.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Reseve enough space in the vector for at least `size` elements.
    pub fn reserve(&mut self, size: usize) {
        let ptr = self.slice.ptr;

        if self.capacity >= size {
            return;
        }

        let mut new_capacity = if self.capacity > 0 { self.capacity } else { 4 };

        while new_capacity < size {
            new_capacity *= 2;
        }

        let new_ptr: NonNull<T> = self.slice.handle.allocate_or_extend(ptr, self.capacity, new_capacity);

        if ptr != new_ptr {
            unsafe {
                ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), self.slice.len());
            }

            self.slice.ptr = new_ptr;
        }

        self.capacity = new_capacity;
    }

    /// Push an element into the vector.
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
            ptr::write(self.slice.ptr.as_ptr().add(self.slice.len()), elem);
        }

        self.slice.len = self.slice.len() + 1;
    }

    /// Resize the vector to hold `len` elements, initialized to `value` if necessary.
    pub fn resize(&mut self, len: usize, value: T)
    where
        T: Clone,
    {
        let old_len = self.slice.len;

        if self.capacity < len {
            self.reserve(len);
        }

        for i in old_len..len.saturating_sub(1) {
            unsafe { ptr::write(self.slice.ptr.as_ptr().add(i), value.clone()) }
        }

        if len > old_len {
            unsafe {
                ptr::write(self.slice.ptr.as_ptr().add(len - 1), value);
            }
        } else if len < old_len {
            unsafe {
                ptr::drop_in_place(&mut self.slice[len..old_len]);
            }
        }

        self.slice.len = len;
    }

    /// Clear the vector.
    pub fn clear(&mut self) {
        unsafe {
            ptr::drop_in_place(&mut self.slice[..]);
        }

        self.slice.len = 0;
    }
}

impl<T: Clone, H: AllocHandle + Clone> Clone for SliceVec<T, H> {
    fn clone(&self) -> Self {
        let mut vec: SliceVec<T, H> = SliceVec::new(self.slice.handle.clone(), self.capacity);

        for i in 0..self.slice.len() {
            unsafe {
                ptr::write(
                    vec.slice.ptr.as_ptr().add(i),
                    (*self.slice.ptr.as_ptr().add(i)).clone(),
                );
            }
        }

        vec.slice.len = self.slice.len();

        vec
    }
}

impl<T: fmt::Debug, H> fmt::Debug for SliceVec<T, H> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.slice.fmt(fmt)
    }
}

impl<T, H> Deref for SliceVec<T, H> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.slice.deref()
    }
}

impl<T, H> DerefMut for SliceVec<T, H> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.slice.deref_mut()
    }
}

impl<T: Eq, H> Eq for SliceVec<T, H> { }

impl<T: PartialEq, H> PartialEq for SliceVec<T, H> {
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}

impl<T: PartialOrd, H> PartialOrd for SliceVec<T, H> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

impl<'a, T: 'a, H> IntoIterator for &'a SliceVec<T, H> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: 'a, H> IntoIterator for &'a mut SliceVec<T, H> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Get the page size of the system we are running on.
#[cfg(unix)]
pub(crate) fn get_page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// Get the page size of the system we are running on.
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

/// Create a virtual memory mapping of size `capacity`.
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

/// Create a virtual memory mapping of size `capacity`.
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

/// Request `capacity` bytes from the system allocator.
pub(crate) fn create_mapping_alloc(capacity: usize) -> *mut u8 {
    unsafe { alloc(Layout::from_size_align_unchecked(capacity, get_page_size())) }
}

/// Destroy a virtual memory mapping.
#[cfg(unix)]
pub(crate) fn destroy_mapping(base: NonNull<u8>, capacity: usize) {
    let res = unsafe { libc::munmap(base.as_ptr() as *mut libc::c_void, capacity) };

    // TODO: Do something on error
    debug_assert_eq!(res, 0);
}

/// Destroy a virtual memory mapping.
#[cfg(windows)]
pub(crate) fn destroy_mapping(base: NonNull<u8>, capacity: usize) {
    use winapi::shared::minwindef::LPVOID;
    use winapi::um::memoryapi::VirtualFree;
    use winapi::um::winnt::MEM_RELEASE;

    let res = unsafe { VirtualFree(base.as_ptr() as LPVOID, 0, MEM_RELEASE) };

    // TODO: Do something on error
    debug_assert_ne!(res, 0);
}

/// Return memory to the system allocator.
pub(crate) fn destroy_mapping_alloc(base: NonNull<u8>, capacity: usize) {
    unsafe {
        let layout = Layout::from_size_align_unchecked(capacity, get_page_size());
        dealloc(base.as_ptr(), layout);
    }
}
