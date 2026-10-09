//! Memory-mapped files for [`sf2synth`](https://github.com/albos-inc/sf2synth)
//! with no dependencies: the system's `mmap` (Unix: macOS, iOS, Linux,
//! Android) or file mapping (Windows), declared here.
//!
//! ```no_run
//! use std::sync::Arc;
//! # fn main() -> std::io::Result<()> {
//! let bytes = sf2synth_mmap::map_file("GeneralUser-GS.sf2")?;
//! // let font = sf2synth::SoundFont::from_shared(bytes)?;
//! # let _ = bytes; Ok(())
//! # }
//! ```
//!
//! A mapped SoundFont costs no heap memory, however large: its samples are
//! read from the file as they play. Pages not read yet are read on first
//! use, possibly on the audio thread, so the whole file is also prefetched
//! in the background. The file must not change while it is mapped.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

/// Shared bytes, as `sf2synth::SoundFont::from_shared` takes them
/// (`sf2synth::SharedBytes`).
pub type SharedBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// The bytes of the file at `path`, memory-mapped (see [`map`]).
pub fn map_file(path: impl AsRef<Path>) -> io::Result<SharedBytes> {
    map(&File::open(path)?)
}

/// The bytes of an open file, memory-mapped. Where mapping isn't available
/// (WebAssembly…) or fails (a file larger than a 32-bit address space…),
/// the file is read into memory instead. The file can be closed afterwards.
pub fn map(file: &File) -> io::Result<SharedBytes> {
    #[cfg(any(unix, windows))]
    {
        if let Ok(Some(map)) = system::Map::new(file) {
            return Ok(Arc::new(map));
        }
    }
    let mut bytes = Vec::new();
    (&*file).read_to_end(&mut bytes)?;
    Ok(Arc::new(bytes))
}

#[cfg(any(unix, windows))]
fn file_length(file: &File) -> io::Result<usize> {
    usize::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file too large to map"))
}

#[cfg(unix)]
mod system {
    use std::ffi::{c_int, c_long, c_void};
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;

    const PROT_READ: c_int = 1;
    const MAP_PRIVATE: c_int = 2;
    const MADV_WILLNEED: c_int = 3;

    unsafe extern "C" {
        // `off_t` is a C `long` on the 64-bit targets and on 32-bit Android.
        fn mmap(
            address: *mut c_void,
            length: usize,
            protection: c_int,
            flags: c_int,
            fd: c_int,
            offset: c_long,
        ) -> *mut c_void;
        fn munmap(address: *mut c_void, length: usize) -> c_int;
        fn madvise(address: *mut c_void, length: usize, advice: c_int) -> c_int;
    }

    /// A read-only private mapping of a whole file.
    pub struct Map {
        address: *mut c_void,
        length: usize,
    }

    // The mapping is read-only and owned by `Map`.
    unsafe impl Send for Map {}
    unsafe impl Sync for Map {}

    impl Map {
        /// The file mapped, or `None` if it's empty.
        pub fn new(file: &File) -> io::Result<Option<Map>> {
            let length = super::file_length(file)?;
            if length == 0 {
                return Ok(None);
            }
            // SAFETY: a fresh read-only private mapping of an open file.
            let address = unsafe { mmap(std::ptr::null_mut(), length, PROT_READ, MAP_PRIVATE, file.as_raw_fd(), 0) };
            if address as isize == -1 {
                return Err(io::Error::last_os_error());
            }
            // Start reading the file in so playback rarely waits for the disk.
            // SAFETY: the range was just mapped. A failure is harmless.
            unsafe { madvise(address, length, MADV_WILLNEED) };
            Ok(Some(Map { address, length }))
        }
    }

    impl AsRef<[u8]> for Map {
        fn as_ref(&self) -> &[u8] {
            // SAFETY: `length` readable bytes stay mapped until drop.
            unsafe { std::slice::from_raw_parts(self.address as *const u8, self.length) }
        }
    }

    impl Drop for Map {
        fn drop(&mut self) {
            // SAFETY: unmaps exactly what `new` mapped, once.
            unsafe { munmap(self.address, self.length) };
        }
    }
}

#[cfg(windows)]
mod system {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    type Handle = *mut c_void;

    const PAGE_READONLY: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x04;

    #[repr(C)]
    struct MemoryRangeEntry {
        address: *mut c_void,
        length: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(
            file: Handle,
            attributes: *mut c_void,
            protection: u32,
            maximum_size_high: u32,
            maximum_size_low: u32,
            name: *const u16,
        ) -> Handle;
        fn MapViewOfFile(mapping: Handle, access: u32, offset_high: u32, offset_low: u32, length: usize)
        -> *mut c_void;
        fn UnmapViewOfFile(address: *const c_void) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetCurrentProcess() -> Handle;
        fn PrefetchVirtualMemory(process: Handle, count: usize, ranges: *const MemoryRangeEntry, flags: u32) -> i32;
    }

    /// A read-only view of a whole file.
    pub struct Map {
        address: *mut c_void,
        length: usize,
    }

    // The view is read-only and owned by `Map`.
    unsafe impl Send for Map {}
    unsafe impl Sync for Map {}

    impl Map {
        /// The file mapped, or `None` if it's empty.
        pub fn new(file: &File) -> io::Result<Option<Map>> {
            let length = super::file_length(file)?;
            if length == 0 {
                return Ok(None);
            }
            // SAFETY: a read-only mapping of an open file; the view keeps it
            // alive once its handle is closed.
            unsafe {
                let mapping = CreateFileMappingW(
                    file.as_raw_handle() as Handle,
                    std::ptr::null_mut(),
                    PAGE_READONLY,
                    0,
                    0,
                    std::ptr::null(),
                );
                if mapping.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let address = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
                let error = io::Error::last_os_error();
                CloseHandle(mapping);
                if address.is_null() {
                    return Err(error);
                }
                // Start reading the file in (Windows 8 and later). A failure
                // is harmless.
                let range = MemoryRangeEntry { address, length };
                PrefetchVirtualMemory(GetCurrentProcess(), 1, &range, 0);
                Ok(Some(Map { address, length }))
            }
        }
    }

    impl AsRef<[u8]> for Map {
        fn as_ref(&self) -> &[u8] {
            // SAFETY: `length` readable bytes stay mapped until drop.
            unsafe { std::slice::from_raw_parts(self.address as *const u8, self.length) }
        }
    }

    impl Drop for Map {
        fn drop(&mut self) {
            // SAFETY: unmaps exactly what `new` mapped, once.
            unsafe { UnmapViewOfFile(self.address) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_whole_file() {
        let path = std::env::temp_dir().join(format!("sf2synth-mmap-{}", std::process::id()));
        let content: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        std::fs::write(&path, &content).unwrap();
        let bytes = map_file(&path).unwrap();
        assert_eq!((*bytes).as_ref(), &content[..]);
        // The file can be closed and the mapping still read.
        let file = File::open(&path).unwrap();
        let from_file = map(&file).unwrap();
        drop(file);
        assert_eq!((*from_file).as_ref(), &content[..]);
        drop((bytes, from_file));
        std::fs::write(&path, b"").unwrap();
        assert!((*map_file(&path).unwrap()).as_ref().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(map_file(&path).is_err());
    }
}
