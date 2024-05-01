//! abstraction of ram frame allocation and mapping

#![allow(unused)]

use alloc::{collections::{BTreeMap, BTreeSet}, sync::Arc};
use lazy_static::lazy_static;
use spin::Mutex;

use super::{frame_alloc, page_table::PTEFlags, FrameTracker, PageTableEntry, PhysPageNum};


// design note: We don't need to feature shared mapping here,
// because it's almost equivalent to mapping to the same file.
// Also no need to cow share a Lazy mapping, as it's always initialized with zeros.

enum MFrame {
    /// not yet allocated
    Lazy,
    /// fully owned, with all possible permissions
    Ownd(FrameTracker),
    /// shared until write, without write permission
    COW(Arc<FrameTracker>),
}

#[derive(Default)]
struct MFrameManager {
    map: BTreeMap<*mut PageTableEntry, MFrame>
}

unsafe impl Send for MFrameManager {}

impl MFrameManager {
    fn is_lazy(&self, pte: *mut PageTableEntry) -> bool {
        match self.map.get(&pte).unwrap() {
            MFrame::Lazy => true,
            _ => false
        }
    }
    fn is_owned(&self, pte: *mut PageTableEntry) -> bool {
        match self.map.get(&pte).unwrap() {
            MFrame::Ownd(_) => true,
            _ => false
        }
    }
    fn is_cow(&self, pte: *mut PageTableEntry) -> bool {
        match self.map.get(&pte).unwrap() {
            MFrame::COW(_) => true,
            _ => false
        }
    }
    /// Track the pte, but not mapped.<br/>
    /// Will invalidate the passed pte.
    fn map_lazy(&mut self, pte: *mut PageTableEntry) {
        self.map.insert(pte, MFrame::Lazy);
        unsafe { pte.as_mut().unwrap().invalidate() };
    }
    /// map the pte immediately to an allocated frame
    fn map_strict(&mut self, pte: *mut PageTableEntry, flags: PTEFlags) {
        self.map_lazy(pte);
        self.load(pte, flags);
    }
    /// load lazy mapping
    fn load(&mut self, pte: *mut PageTableEntry, flags: PTEFlags) {
        let entry = self.map.get_mut(&pte).unwrap();
        match entry {
            MFrame::Ownd(_) => {}
            MFrame::COW(_) => panic!("cannot load a COW mapping"),
            MFrame::Lazy => {
                let frame = frame_alloc().unwrap();
                unsafe {
                    *pte = PageTableEntry::new(frame.ppn, flags);
                }
                *entry = MFrame::Ownd(frame);
            }
        }
    }
    /// Own a COW mapping, thus is named `cown`.
    fn cown(&mut self, pte: *mut PageTableEntry) {
        let entry = self.map.get_mut(&pte).unwrap();
        match entry {
            MFrame::Lazy => panic!("cannot own a lazy mapping"),
            MFrame::Ownd(_) => {}
            MFrame::COW(shared) => {
                let mut flags = unsafe { pte.as_mut().unwrap().flags() };
                #[allow(invalid_value)]
                match Arc::try_unwrap(unsafe { core::mem::replace(shared, core::mem::zeroed()) }) {
                    Err(arc) => {
                        let frame = frame_alloc().unwrap();
                        frame.ppn.get_bytes_array().copy_from_slice(&arc.ppn.get_bytes_array());
                        unsafe {
                            *pte = PageTableEntry::new(frame.ppn, flags);
                            pte.as_mut().unwrap().revive_writability();
                            core::mem::forget(core::mem::replace(shared, arc)); // forget the invalid value
                        }
                        *entry = MFrame::Ownd(frame);
                    }
                    Ok(frame) => {
                        unsafe {
                            *pte = PageTableEntry::new(frame.ppn, flags);
                            pte.as_mut().unwrap().revive_writability();
                            core::mem::forget(core::mem::replace(entry, MFrame::Ownd(frame))); // forget the invalid value
                        }
                    }
                }
            }
        }
    }
    /// Untrack and invalidate the mapping.
    fn unmap(&mut self, pte: *mut PageTableEntry) {
        match self.map.remove(&pte).expect("Cannot unmap a not tracked map") {
            MFrame::Lazy => {}
            MFrame::Ownd(_) | MFrame::COW(_) => unsafe { pte.as_mut().unwrap().invalidate() },
        }
    }
    /// COW share the mapping of `pte_origin`.
    fn share_cow(&mut self, pte_origin: *mut PageTableEntry, pte_borrow: *mut PageTableEntry) {
        let entry = self.map.get_mut(&pte_origin).unwrap();
        match entry {
            MFrame::Lazy => {
                self.map.insert(pte_borrow, MFrame::Lazy);
            }
            MFrame::Ownd(frame) => {
                unsafe {
                    let mut flags = pte_origin.as_ref().unwrap().flags();
                    flags.set(PTEFlags::W, false);
                    let shared = Arc::new(core::mem::replace(frame, core::mem::zeroed()));
                    *pte_origin = PageTableEntry::new(shared.ppn, flags);
                    *pte_borrow = PageTableEntry::new(shared.ppn, flags);
                    core::mem::forget(core::mem::replace(entry, MFrame::COW(shared.clone()))); // forget the invalid value
                    self.map.insert(pte_borrow, MFrame::COW(shared));
                }
            }
            MFrame::COW(shared) => {
                unsafe {
                    let mut flags = pte_origin.as_ref().unwrap().flags();
                    *pte_borrow = PageTableEntry::new(shared.ppn, flags);
                    let shared = shared.clone();
                    self.map.insert(pte_borrow, MFrame::COW(shared));
                }
            }
        }
    }
}


lazy_static! {
    static ref MFRAME_MANAGER: Mutex<MFrameManager> = Mutex::new(MFrameManager::default());
}

pub struct MFrameHandle {
    pte: *mut PageTableEntry,
}

impl Drop for MFrameHandle {
    fn drop(&mut self) {
        MFRAME_MANAGER.lock().unmap(self.pte as *mut PageTableEntry);
    }
}

impl MFrameHandle {
    pub fn map_lazy(pte: *mut PageTableEntry) -> Self {
        MFRAME_MANAGER.lock().map_lazy(pte);
        Self {
            pte
        }
    }
    pub fn map_strict(pte: *mut PageTableEntry, flags: PTEFlags) -> Self {
        MFRAME_MANAGER.lock().map_strict(pte, flags);
        Self {
            pte
        }
    }
    pub fn load(&self, flags: PTEFlags) {
        MFRAME_MANAGER.lock().load(self.pte, flags);
    }
    pub fn cown(&self) {
        MFRAME_MANAGER.lock().cown(self.pte);
    }
    pub fn share_cow(&self, other: *mut PageTableEntry) -> MFrameHandle {
        MFRAME_MANAGER.lock().share_cow(self.pte, other);
        MFrameHandle {
            pte: other
        }
    }
    pub fn executable(&self) -> bool {
        unsafe { self.pte.as_ref().unwrap().executable() }
    }
    pub fn writable(&self) -> bool {
        unsafe { self.pte.as_ref().unwrap().writable() }
    }
    pub fn readable(&self) -> bool {
        unsafe { self.pte.as_ref().unwrap().readable() }
    }
    pub fn is_lazy(&self) -> bool {
        MFRAME_MANAGER.lock().is_lazy(self.pte)
    }
    pub fn is_owned(&self) -> bool {
        MFRAME_MANAGER.lock().is_owned(self.pte)
    }
    pub fn is_cow(&self) -> bool {
        MFRAME_MANAGER.lock().is_cow(self.pte)
    }
}
