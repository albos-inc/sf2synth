//! Memory-mapping a file without dependencies: POSIX `mmap` declared here
//! (it's in the C library every Rust program on these systems links).

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use sf2synth::SharedBytes;

/// The bytes of a file: memory-mapped where the system allows it (Unix:
/// macOS, iOS, Linux, Android), else read into memory.
pub fn map_file(path: impl AsRef<Path>) -> io::Result<SharedBytes> {
    let file = File::open(path)?;
    #[cfg(unix)]
    {
        if let Some(map) = unix::Map::new(&file)? {
            return Ok(Arc::new(map));
        }
    }
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut &file, &mut bytes)?;
    Ok(Arc::new(bytes))
}

#[cfg(unix)]
mod unix {
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
            let length = usize::try_from(file.metadata()?.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file too large to map"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_whole_file() {
        let path = std::env::temp_dir().join(format!("sf2synth-map-{}", std::process::id()));
        let content: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        std::fs::write(&path, &content).unwrap();
        let bytes = map_file(&path).unwrap();
        assert_eq!((*bytes).as_ref(), &content[..]);
        drop(bytes);
        std::fs::write(&path, b"").unwrap();
        assert!((*map_file(&path).unwrap()).as_ref().is_empty());
        std::fs::remove_file(&path).unwrap();
    }
}
