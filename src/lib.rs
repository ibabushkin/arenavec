use std::alloc::{alloc, dealloc, Layout};
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
pub struct Arena(InnerRef, bool);

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

/// An arena allocated, sequential, resizable vector
///
/// Since the arena does not support resizing, or freeing memory, this implementation just
/// creates new slices as necessary and leaks the previous arena allocation, trading memory
/// for speed.
pub struct SliceVec<T> {
    slice: Slice<T>,
    // owo what's this
    capacity: usize,
}

/// An iterator over a sequence of arena-allocated objects
#[derive(Debug)]
pub struct SliceIter<'a, T: 'a> {
    ptr: *const T,
    end: *const T,
    _marker: marker::PhantomData<&'a T>,
}

/// An iterator over a mutable sequence of arena-allocated objects
#[derive(Debug)]
pub struct SliceIterMut<'a, T: 'a> {
    ptr: *mut T,
    end: *mut T,
    _marker: marker::PhantomData<&'a T>,
}

impl Arena {
    #[cfg(unix)]
    fn get_page_size() -> usize {
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    #[cfg(windows)]
    fn get_page_size() -> usize {
        use std::mem;
        use winapi::um::sysinfoapi::GetSystemInfo;

        unsafe {
            let mut info = mem::zeroed();
            GetSystemInfo(&mut info);

            info.dwPageSize as usize
        }
    }

    #[cfg(unix)]
    fn create_mapping(capacity: usize) -> *mut u8 {
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
    fn create_mapping(capacity: usize) -> *mut u8 {
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

    /// Create an `Arena` with specified capacity.
    ///
    /// Capacity must be a power of 2. The capacity cannot be grown after the fact.
    pub fn init_capacity(cap: usize) -> Arena {
        let head = Arena::create_mapping(cap);
        let pos = Cell::new(0);

        Arena(
            InnerRef {
                inner: Rc::new(Inner { head, pos, cap }),
            },
            false,
        )
    }

    fn create_mapping_alloc(capacity: usize) -> *mut u8 {
        unsafe {
            alloc(Layout::from_size_align_unchecked(
                capacity,
                Arena::get_page_size(),
            ))
        }
    }

    pub fn init_capacity_alloc(cap: usize) -> Arena {
        let head = Arena::create_mapping_alloc(cap);
        let pos = Cell::new(0);

        Arena(
            InnerRef {
                inner: Rc::new(Inner { head, pos, cap }),
            },
            true,
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

#[cfg(unix)]
impl Drop for Arena {
    fn drop(&mut self) {
        if self.1 {
            unsafe {
                let layout =
                    Layout::from_size_align_unchecked(self.0.inner.cap, Arena::get_page_size());
                dealloc(self.inner.head, layout)
            }
        } else {
            let res = unsafe { libc::munmap(self.inner.head as *mut libc::c_void, self.inner.cap) };

            // TODO: Do something on error
            debug_assert_eq!(res, 0);
        }
    }
}

#[cfg(windows)]
impl Drop for Arena {
    fn drop(&mut self) {
        if self.1 {
            unsafe {
                let layout =
                    Layout::from_size_align_unchecked(self.0.capacity, Arena::get_page_size());
                dealloc(self.inner.head, layout)
            }
        } else {
            use winapi::shared::minwindef::LPVOID;
            use winapi::um::memoryapi::VirtualFree;
            use winapi::um::winnt::MEM_RELEASE;

            let res = unsafe { VirtualFree(self.inner.head as LPVOID, 0, MEM_RELEASE) };

            // TODO: Do something on error
            debug_assert_ne!(res, 0);
        }
    }
}

impl InnerRef {
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

impl<T> Slice<T> {
    pub fn new(inner: InnerRef, len: usize) -> Self
    where
        T: Default,
    {
        let ptr: *mut T = inner.allocate(len);

        for i in 0..len {
            unsafe {
                ptr::write(ptr.add(i), T::default());
            }
        }

        Slice {
            ptr,
            len,
            _inner: inner,
        }
    }

    pub fn iter(&self) -> SliceIter<T> {
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

    pub fn iter_mut(&mut self) -> SliceIterMut<T> {
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
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
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

impl<T> SliceVec<T> {
    pub fn new(inner: InnerRef, capacity: usize) -> Self {
        let ptr: *mut T = if capacity == 0 {
            ptr::NonNull::dangling().as_ptr()
        } else {
            inner.allocate(capacity)
        };

        SliceVec {
            slice: Slice {
                ptr,
                len: 0,
                _inner: inner.clone(),
            },
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn iter(&self) -> SliceIter<T> {
        self.slice.iter()
    }

    pub fn iter_mut(&mut self) -> SliceIterMut<T> {
        self.slice.iter_mut()
    }

    pub fn reserve(&mut self, size: usize) {
        if self.capacity >= size {
            return;
        }

        let mut new_capacity = if self.capacity > 0 { self.capacity } else { 4 };

        while new_capacity < size {
            new_capacity *= 2;
        }

        let ptr: *mut T =
            self.slice
                ._inner
                .allocate_or_extend(self.slice.ptr, self.capacity, new_capacity);

        if !self.slice.ptr.is_null() && ptr != self.slice.ptr {
            unsafe {
                ptr::copy_nonoverlapping(self.slice.ptr, ptr, self.slice.len);
            }

            self.slice.ptr = ptr;
        }

        self.capacity = new_capacity;
    }

    pub fn push(&mut self, elem: T) {
        if self.slice.len == self.capacity {
            let new_capacity = if self.capacity == 0 {
                4
            } else {
                self.capacity * 2
            };
            let ptr: *mut T =
                self.slice
                    ._inner
                    .allocate_or_extend(self.slice.ptr, self.capacity, new_capacity);

            if !self.slice.ptr.is_null() && self.slice.ptr != ptr {
                unsafe {
                    ptr::copy_nonoverlapping(self.slice.ptr, ptr, self.slice.len);
                }

                self.slice.ptr = ptr;
            }

            self.capacity = new_capacity;
        }

        unsafe {
            ptr::write(self.slice.ptr.add(self.slice.len), elem);
        }

        self.slice.len += 1;
    }

    pub fn resize(&mut self, len: usize, value: T)
    where
        T: Clone,
    {
        if self.capacity < len {
            self.reserve(len);
        }

        for i in self.slice.len..len.saturating_sub(1) {
            unsafe { ptr::write(self.slice.ptr.add(i), value.clone()) }
        }

        if len > self.slice.len {
            unsafe {
                ptr::write(self.slice.ptr.add(len - 1), value);
            }
        }

        self.slice.len = len;
    }

    pub fn clear(&mut self) {
        self.slice.len = 0;
    }
}

impl<T: Clone> Clone for SliceVec<T> {
    fn clone(&self) -> Self {
        let ptr: *mut T = self.slice._inner.allocate(self.capacity);

        for i in 0..self.slice.len {
            unsafe {
                ptr::write(ptr.add(i), (*self.slice.ptr.add(i)).clone());
            }
        }

        SliceVec {
            slice: Slice {
                ptr,
                len: self.slice.len,
                _inner: self.slice._inner.clone(),
            },
            capacity: self.capacity,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for SliceVec<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        self.slice.fmt(fmt)
    }
}

impl<T> Deref for SliceVec<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.slice.deref()
    }
}

impl<T> DerefMut for SliceVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.slice.deref_mut()
    }
}

impl<T: Eq> Eq for SliceVec<T> {}

impl<T: PartialEq> PartialEq for SliceVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref())
    }
}

impl<T: PartialOrd> PartialOrd for SliceVec<T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

/* impl<T> Default for SliceVec<T> {
    fn default() -> Self {
        SliceVec::new(0)
    }
}

#[cfg(feature = "serde")]
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
} */

impl<'a, T> IntoIterator for &'a SliceVec<T> {
    type Item = &'a T;
    type IntoIter = SliceIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut SliceVec<T> {
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
