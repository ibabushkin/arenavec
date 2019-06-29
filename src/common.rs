use std::alloc::{alloc, dealloc, Layout};
use std::ptr;

#[derive(Debug)]
pub enum ArenaBacking {
    MemoryMap,
    SystemAllocation,
}

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
    unsafe {
        alloc(Layout::from_size_align_unchecked(
            capacity,
            get_page_size(),
        ))
    }
}

#[cfg(unix)]
pub(crate) fn destroy_mapping(base: *mut u8, capacity: usize) {
    let res =
        unsafe { libc::munmap(base as *mut libc::c_void, capacity) };

    // TODO: Do something on error
    debug_assert_eq!(res, 0);
}

#[cfg(windows)]
pub(crate) fn destroy_mapping(base: *mut u8, capacity: usize) {
    if self.1 {
        unsafe {
            let layout =
                Layout::from_size_align_unchecked(self.0.capacity, common::get_page_size());
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

pub(crate) fn destroy_mapping_alloc(base: *mut u8, capacity: usize) {
    unsafe {
        let layout =
            Layout::from_size_align_unchecked(capacity, get_page_size());
        dealloc(base, layout);
    }
}
