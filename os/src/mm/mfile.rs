//! abstraction of memory-file manipulation,
//! for both memory-mapped-file and page swapping

#![allow(unused)]

use alloc::{collections::{BTreeMap, BTreeSet}, sync::Arc, vec::Vec};
use easy_fs::Inode;
use lazy_static::lazy_static;
use spin::Mutex;

use super::{frame_alloc, page_table::PTEFlags, FrameTracker, PageTableEntry, PhysPageNum};


#[derive(Clone)]
pub struct FilePos {
    inode: Arc<Inode>,
    offset: usize
}

/// CRITICAL: we don't care device id, currently
impl Ord for FilePos {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let idl = self.inode.get_inode_id();
        let idr = other.inode.get_inode_id();
        let r1 = idl.cmp(&idr);
        if r1 == core::cmp::Ordering::Equal {
            self.offset.cmp(&other.offset)
        } else {
            r1
        }
    }
}

impl PartialOrd for FilePos {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for FilePos {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == core::cmp::Ordering::Equal
    }
}

impl Eq for FilePos {}

struct MFile {
    pos: FilePos,
    frame: Mutex<Option<FrameTracker>>
}

impl Ord for MFile {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.pos.cmp(&other.pos)
    }
}

impl PartialOrd for MFile {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for MFile {
    fn eq(&self, other: &Self) -> bool {
        self.pos == other.pos
    }
}

impl Eq for MFile {}

impl FilePos {
    fn write(&self, buf: &[u8]) -> usize {
        self.inode.write_at(self.offset, buf)
    }
    fn read(&self, buf: &mut [u8]) -> usize {
        self.inode.read_at(self.offset, buf)
    }
}

impl MFile {
    fn loaded(&self) -> bool {
        self.frame.lock().is_some()
    }
    /// sync all data and then release the held ram frame
    fn sync(&self) {
        let mut lock = self.frame.lock();
        match lock.take() {
            None => {}
            Some(frame) => {
                self.pos.write(&frame.ppn.get_bytes_array());
                drop(frame);
            }
        }
    }
    /// load file into memory, does nothing if already loaded
    fn load(&self) -> PhysPageNum {
        let mut lock = self.frame.lock();
        match &*lock {
            Some(frame) => frame.ppn,
            None => {
                let frame = frame_alloc().unwrap(); // assume enough
                let ppn = frame.ppn;
                *lock = Some(frame);
                ppn
            }
        }
    }
}


lazy_static! {
    static ref MFILE_MANAGER: Mutex<MFileManager> = Mutex::new(MFileManager::default());
}

/// this type does not perform any flush
#[derive(Default)]
struct MFileManager {
    files: BTreeSet<Arc<MFile>>,
    map: BTreeMap<*mut PageTableEntry, Arc<MFile>>,
    rmap: BTreeMap<FilePos, BTreeSet<*mut PageTableEntry>>,
}

unsafe impl Send for MFileManager {}

impl MFileManager {
    /// After calling `map`, until calling `unmap`, the pte should **NOT** be modified by any other mechanism.
    fn map(&mut self, pte: *mut PageTableEntry, inode: Arc<Inode>, offset: usize) {
        let pos = FilePos {
            inode,
            offset,
        };
        let file = Arc::new(MFile {
            pos: pos.clone(),
            frame: Mutex::new(None)
        });
        if !self.files.contains(&file) {
            self.files.insert(file.clone());
            self.map.insert(pte, file);
            self.rmap.entry(pos).or_default().insert(pte);
        } else {
            let file = self.files.get(&file).unwrap(); // cannot fail
            self.map.insert(pte, file.clone());
            self.rmap.entry(pos).or_default().insert(pte);
        }
    }
    /// This function will invalidate the pte, but the file will remain loaded until `slim` is called.<br/>
    /// Make sure to call this function when the pte is no longer available, e.g. when the process exits, or else memory leaks on kernel heap.<br/>
    /// In the worst case, will lead to data corruption on the ram frame at which the pte is originally located.
    fn unmap(&mut self, pte: *mut PageTableEntry) {
        if let Some(file) = self.map.remove(&pte) {
            if file.loaded() {
                unsafe { pte.as_mut().unwrap().invalidate() };
            }
            self.rmap.get_mut(&file.pos).unwrap().remove(&pte);
        }
    }
    /// Call this function to sync and release all loaded but unused files.<br/>
    /// Mainly used to free up ram frames as well as kernel heap, without hurting performance.
    fn slim(&mut self) {
        let recycle = self.files.iter()
            .filter(|x|Arc::strong_count(x) == 1)
            .map(Arc::clone)
            .collect::<Vec<Arc<MFile>>>();
        for file in recycle.into_iter() {
            file.sync(); // archive the data
            self.files.remove(&file);
        }
    }
    /// Load the file and set ppn, caller should pass `flags` with `PTEFlags::V` to ensure the mapping is validated.<br/>
    /// Only the passed pte is modified, although there may be other ptes tracked which are also mapped to the same file and offset.<br/>
    /// Does **NOT** flush in all sense, so caller should care by itself.<br/>
    /// 
    /// This function is intended to be called on every individual page fault of different processes,<br/>
    /// which is safe since rCore will have `SFENCE.VMA` at every kernel-user switch.
    fn load(&mut self, pte: *mut PageTableEntry, flags: PTEFlags) {
        let map = self.map.get(&pte).unwrap();
        let ppn = map.load();
        unsafe {
            *pte = PageTableEntry::new(ppn, flags);
        }
    }
}

pub struct MFileHandle<'a> {
    pte: &'a mut PageTableEntry,
}

impl<'a> Drop for MFileHandle<'a> {
    fn drop(&mut self) {
        MFILE_MANAGER.lock().unmap(self.pte as *mut PageTableEntry);
    }
}

impl<'a> MFileHandle<'a> {
    /// Create a file mapping handle, which on drop will unmap itself.
    pub fn map(pte: &'a mut PageTableEntry, inode: Arc<Inode>, offset: usize) -> Self {
        MFILE_MANAGER.lock().map(pte as *mut PageTableEntry, inode, offset);
        Self {
            pte,
        }
    }
    /// This function is intended to be called on every individual page fault of different processes.
    pub fn load(&mut self, flags: PTEFlags) {
        MFILE_MANAGER.lock().load(self.pte as *mut PageTableEntry, flags);
    }
}
