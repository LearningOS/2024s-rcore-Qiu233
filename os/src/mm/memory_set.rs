//! Implementation of [`MapArea`] and [`MemorySet`].
use super::mfile::MFileHandle;
use super::mframe::MFrameHandle;
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::{StepByOne, VPNRange};
use crate::config::{MEMORY_END, MMIO, PAGE_SIZE, TRAMPOLINE, TRAP_CONTEXT_BASE, USER_STACK_SIZE};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use easy_fs::Inode;
use spin::Mutex;
use core::arch::asm;
use lazy_static::*;
use riscv::register::satp;

extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss_with_stack();
    fn ebss();
    fn ekernel();
    fn strampoline();
}

lazy_static! {
    /// The kernel's initial memory mapping(kernel address space)
    pub static ref KERNEL_SPACE: Mutex<MemorySet> = Mutex::new(MemorySet::new_kernel());
}

/// the kernel token
pub fn kernel_token() -> usize {
    KERNEL_SPACE.lock().token()
}

/// address space
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}

impl MemorySet {
    /// Create a new empty `MemorySet`.
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    /// Get the page table token
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// Assume that no conflicts.
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) {
        self.new_area(
            MapArea::map_framed(
                &self.page_table, start_va.floor(), end_va.ceil(), permission)
        );
    }
    /// remove a area
    pub fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some((idx, _)) = self
            .areas
            .iter_mut()
            .enumerate()
            .find(|(_, area)| area.vpn_range.get_start() == start_vpn)
        {
            self.areas.remove(idx);
        }
    }
    /// Add a new MapArea into this MemorySet.
    /// Assuming that there are no conflicts in the virtual address
    /// space.
    fn new_area(&mut self, map_area: MapArea) {
        self.areas.push(map_area);
    }
    /// Mention that trampoline is not collected by areas.
    fn map_trampoline(&self) {
        let pte = self.page_table.create_force(VirtAddr::from(TRAMPOLINE).into());
        *pte = PageTableEntry::new(
            PhysPageNum::from(PhysAddr::from(strampoline as usize)), 
            PTEFlags::R | PTEFlags::X | PTEFlags::V)
    }
    /// Without kernel stacks.
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map kernel sections
        info!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        info!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        info!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        info!(
            ".bss [{:#x}, {:#x})",
            sbss_with_stack as usize, ebss as usize
        );
        info!("mapping .text section");
        memory_set.new_area(
            MapArea::map_identity(
                &memory_set.page_table,
                (stext as usize).into(),
                (etext as usize).into(),
                MapPermission::R | MapPermission::X,
            ),
        );
        info!("mapping .rodata section");
        memory_set.new_area(
            MapArea::map_identity(
                &memory_set.page_table,
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapPermission::R,
            ),
        );
        info!("mapping .data section");
        memory_set.new_area(
            MapArea::map_identity(
                &memory_set.page_table,
                (sdata as usize).into(),
                (edata as usize).into(),
                MapPermission::R | MapPermission::W,
            ),
        );
        info!("mapping .bss section");
        memory_set.new_area(
            MapArea::map_identity(
                &memory_set.page_table,
                (sbss_with_stack as usize).into(),
                (ebss as usize).into(),
                MapPermission::R | MapPermission::W,
            ),
        );
        info!("mapping physical memory");
        memory_set.new_area(
            MapArea::map_identity(
                &memory_set.page_table,
                (ekernel as usize).into(),
                MEMORY_END.into(),
                MapPermission::R | MapPermission::W,
            ),
        );
        info!("mapping memory-mapped registers");
        for pair in MMIO {
            memory_set.new_area(
                MapArea::map_identity(
                    &memory_set.page_table,
                    (*pair).0.into(),
                    ((*pair).0 + (*pair).1).into(),
                    MapPermission::R | MapPermission::W,
                ),
            );
        }
        memory_set
    }
    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp_base and entry point.
    pub fn from_elf(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map program headers of elf, with U flag
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_vpn = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let map_area = MapArea::map_framed(
                    &memory_set.page_table,
                    // these addresses are guaranteed to be multiple of page size, according elf format
                    start_va.floor(), end_va.ceil(),
                    map_perm).then_load_all();
                max_end_vpn = map_area.vpn_range.get_end();
                map_area.copy_data(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize], &memory_set.page_table);
                memory_set.new_area(map_area);
            }
        }
        // map user stack with U flags
        let max_end_va: VirtAddr = max_end_vpn.into();
        let mut user_stack_bottom: usize = max_end_va.into();
        // guard page
        user_stack_bottom += PAGE_SIZE;
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        memory_set.new_area(
            MapArea::map_framed(
                &memory_set.page_table,
                user_stack_bottom.into(),
                user_stack_top.into(),
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
        );
        // used in sbrk
        memory_set.new_area(
            MapArea::map_framed(
                &memory_set.page_table,
                user_stack_top.into(),
                user_stack_top.into(),
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
        );
        // map TrapContext
        memory_set.new_area(
            MapArea::map_framed(
                &memory_set.page_table,
                TRAP_CONTEXT_BASE.into(),
                TRAMPOLINE.into(),
                MapPermission::R | MapPermission::W,
            ).then_load_all(),
        );
        (
            memory_set,
            user_stack_top,
            elf.header.pt2.entry_point() as usize,
        )
    }

    
    /// only returns true if the mapping is COW and owning succeeds
    pub fn cown(&mut self, vpn: VirtPageNum) -> bool {
        if let Some(area) = self.areas.iter().find(|x|x.vpn_range.contains(&vpn)) {
            area.data_frames.get(&vpn).unwrap().cown()
        } else {
            false
        }
    }

    /// Create a new address space by copy code&data from a exited process's address space.
    pub fn fork(&self) -> Self {
        let mut memory_set = Self::new_bare();
        memory_set.map_trampoline();
        for area in self.areas.iter() {
            memory_set.new_area(area.fork(&memory_set.page_table));
        }
        memory_set
    }
    /// Change page table by writing satp CSR Register.
    pub fn activate(&self) {
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp);
            asm!("sfence.vma");
        }
    }
    /// Translate a virtual page number to a page table entry
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }

    ///Remove all `MapArea`
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }

    /// shrink the area to new_end
    #[allow(unused)]
    pub fn shrink_to(&mut self, start: VirtAddr, new_end: VirtAddr) -> bool {
        if let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.vpn_range.get_start() == start.floor())
        {
            area.shrink_to(&mut self.page_table, new_end.ceil());
            true
        } else {
            false
        }
    }

    /// append the area to new_end
    #[allow(unused)]
    pub fn append_to(&mut self, start: VirtAddr, new_end: VirtAddr) -> bool {
        if let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.vpn_range.get_start() == start.floor())
        {
            area.append_to(&mut self.page_table, new_end.ceil());
            true
        } else {
            false
        }
    }
    /// test if there are mapped area whitin the given range.<br/>
    /// note that this doesn't check mappings which are not tracked by `MapArea`s
    fn has_mapped(&self, range: VPNRange) -> bool {
        self.areas.iter().any(|x|x.vpn_range.intersects(&range))
    }

    /// test if there are unmapped area whitin the given range.<br/>
    /// note that this doesn't check mappings which are not tracked by `MapArea`s
    fn has_unmapped(&self, range: VPNRange) -> bool {
        let count = self.areas.iter().map(|x|{
            let (_, _, rem) = x.vpn_range.exclude(&range);
            rem.into_iter().count()
        }).sum::<usize>();
        
        let expected = range.into_iter().count();
        count != expected
    }

    /// Gets whether the specified virtual page is critical and thus cannot be unmapped.
    fn is_critical(vpn: VirtPageNum) -> bool {
        if vpn == VirtPageNum::from(VirtAddr::from(TRAMPOLINE)) {
            return true;
        } else if vpn == VirtPageNum::from(VirtAddr::from(TRAP_CONTEXT_BASE)) {
            return true;
        }
        return false;
    }

    /// Try to map virtual address range, with memory not allocated until actual use.
    pub fn mmap(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) -> isize  {
        let area = MapArea::map_framed(&self.page_table, start_va.floor(), end_va.ceil(), permission);
        if area.vpn_range.into_iter().any(|x|Self::is_critical(x)) {
            return -1;
        }
        if self.has_mapped(area.vpn_range) {
            return -1;
        }
        self.new_area(
            area,
        );
        0
    }

    /// Try to unmap virtual address range, except for **critical mappings** such as `TRAMPOLINE` and `TRAP_CONTEXT_BASE`.
    /// One area will be split into two if it's unmapped in the middle.
    pub fn munmap(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
    ) -> isize  {
        let target_range = VPNRange::new(start_va.floor(), end_va.ceil());
        if target_range.into_iter().any(|x|Self::is_critical(x)) {
            return -1;
        }
        if self.has_unmapped(target_range) {
            return -1;
        }
        let areas = core::mem::take(&mut self.areas);
        for area in areas.into_iter() {
            // compute ranges
            let (l, _, rem) = area.vpn_range.exclude(&target_range);
            if rem.is_empty() { // nothing to remove in this area, push and skip
                self.areas.push(area);
                continue;
            }
            let (larea, rarea) = area.split(l.get_end());
            let (marea, rarea) = rarea.split(rem.get_end());
            // now `larea`/`rarea` are the left/right parts to preserve, respectively
            // if some of them are empty, then there's no need to push back
            if !larea.vpn_range.is_empty() {
                self.areas.push(larea);
            }
            if !rarea.vpn_range.is_empty() {
                self.areas.push(rarea);
            }
            drop(marea); // will unmap all when dropped
        }
        0
    }
}
/// map area structure, controls a contiguous piece of virtual memory
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, Page>,
    map_type: MapType,
    map_perm: MapPermission,
    file: Option<Arc<Inode>>,
    file_offset: usize
}

impl MapArea {
    /// Merge two adjacent areas into one.
    fn merge(self, other: Self) -> Self {
        assert!(self.vpn_range.get_end() == other.vpn_range.get_start());
        assert!(self.map_type == other.map_type);
        assert!(self.map_perm == other.map_perm);
        assert!(self.file == other.file); // how about device id?
        let len_left = self.vpn_range.into_iter().count();
        let len_right = other.vpn_range.into_iter().count();
        match self.map_type {
            MapType::Identity => {}
            MapType::Framed => {}
            MapType::FileShared => {
                assert!(self.file_offset + len_left * PAGE_SIZE == other.file_offset);
            }
        }
        if len_left == 0 {
            other
        } else if len_right == 0 {
            self
        } else {
            let mut result = self;
            result.vpn_range = VPNRange::new(result.vpn_range.get_start(), other.vpn_range.get_end());
            result.data_frames.extend(other.data_frames.into_iter()); // mappings are merged here
            result
        }
    }
    /// split the given area into two, with the same type and permission.<br/>
    /// `(self[start..vpn), self[vpn..end))` is returned
    fn split(self, vpn: VirtPageNum) -> (Self, Self) {
        let mut other = Self {
            vpn_range: VPNRange::new(vpn, vpn),
            data_frames: BTreeMap::new(),
            map_type: self.map_type, map_perm: self.map_perm,
            file: self.file.clone(),
            file_offset: self.file_offset,
        };
        if vpn <= self.vpn_range.get_start() {
            return (other, self);
        } else if vpn >= self.vpn_range.get_end() {
            return (self, other);
        } else {
            let mut mapl = BTreeMap::new();
            let mut mapr = BTreeMap::new();
            // collect `FrameTracker`s into different maps, according to their vpn
            for (i, frame) in self.data_frames.into_iter() { // self.data_frames moved here
                if i < vpn {
                    mapl.insert(i, frame);
                } else {
                    mapr.insert(i, frame);
                }
            }
            let left_half_size = (vpn.0 - self.vpn_range.get_start().0) * PAGE_SIZE;
            let left = Self {
                vpn_range: VPNRange::new(self.vpn_range.get_start(), vpn),
                data_frames: mapl,
                map_type: self.map_type,
                map_perm: self.map_perm,
                file: self.file.clone(),
                file_offset: self.file_offset,
            };
            other = Self {
                vpn_range: VPNRange::new(vpn, self.vpn_range.get_end()),
                data_frames: mapr,
                map_type: self.map_type,
                map_perm: self.map_perm,
                file: self.file.clone(),
                file_offset: self.file_offset + left_half_size,
            };
            return (left, other);
        }
    }
    fn new(
        page_table: &PageTable,
        start_vpn: VirtPageNum,
        end_vpn: VirtPageNum,
        map_type: MapType,
        map_perm: MapPermission,
        file: Option<Arc<Inode>>,
        file_offset: usize,
        data_frames_override: Option<BTreeMap<VirtPageNum, Page>>
    ) -> Self {
        match map_type {
            MapType::Identity | MapType::Framed => assert!(file.is_none() && file_offset == 0),
            MapType::FileShared => assert!(file.is_some() && file_offset % PAGE_SIZE == 0),
        }
        let vpn_range = VPNRange::new(start_vpn, end_vpn);
        let data_frames = match data_frames_override {
            Some(data_frames) => data_frames,
            None => {
                let mut data_frames = BTreeMap::new();
                match map_type {
                    MapType::Identity => {
                        for vpn in vpn_range {
                            let pte = page_table.create_force(vpn);
                            let pte_flags: PTEFlags = map_perm.into();
                            data_frames.insert(vpn, Page::Identity(MIdentityHandle::map(pte, vpn, pte_flags | PTEFlags::V)));
                        }
                    }
                    MapType::Framed => {
                        for vpn in vpn_range {
                            let page = Page::framed_lazy(page_table.create_force(vpn));
                            data_frames.insert(vpn, page);
                        }
                    }
                    MapType::FileShared => {
                        for vpn in vpn_range {
                            let offset = (vpn.0 - start_vpn.0) * PAGE_SIZE;
                            let file = file.clone().unwrap(); // cannot fail
                            let page = Page::file_shared(page_table.create_force(vpn), file, offset);
                            data_frames.insert(vpn, page);
                        }
                    }
                }
                data_frames
            }
        };
        Self {
            vpn_range,
            data_frames,
            map_type,
            map_perm,
            file,
            file_offset,
        }
    }

    fn map_identity(
        page_table: &PageTable,
        start_vpn: VirtPageNum,
        end_vpn: VirtPageNum,
        map_perm: MapPermission
    ) -> Self {
        Self::new(page_table, start_vpn, end_vpn, MapType::Identity, map_perm, None, 0, None)
    }
    fn map_framed(
        page_table: &PageTable,
        start_vpn: VirtPageNum,
        end_vpn: VirtPageNum,
        map_perm: MapPermission
    ) -> Self {
        Self::new(page_table, start_vpn, end_vpn, MapType::Framed, map_perm, None, 0, None)
    }
    #[allow(unused)]
    pub fn shrink_to(&mut self, page_table: &mut PageTable, new_end: VirtPageNum) {
        let original = core::mem::replace(self, unsafe { core::mem::zeroed() });
        let (left, _) = original.split(new_end); // right half is dropped
        core::mem::forget(core::mem::replace(self, left));
    }
    #[allow(unused)]
    pub fn append_to(&mut self, page_table: &mut PageTable, new_end: VirtPageNum) {
        let len = self.vpn_range.into_iter().count();
        let delta = Self::new(page_table, 
            self.vpn_range.get_end(), new_end, 
            self.map_type, self.map_perm,
            self.file.clone(), 
            self.file_offset + len * PAGE_SIZE, None);
        let original = core::mem::replace(self, unsafe { core::mem::zeroed() });
        let merged = original.merge(delta);
        core::mem::forget(core::mem::replace(self, merged));
    }
    /// data: start-aligned but maybe with shorter length
    /// assume that all frames were cleared before
    pub fn copy_data(&self, data: &[u8], page_table: &PageTable) { //TODO: remove page_table
        assert!(self.map_type != MapType::Identity);
        self.load_all();
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
    }
    pub fn load_one(&self, vpn: VirtPageNum) {
        let flags: PTEFlags = self.map_perm.into();
        self.data_frames.get(&vpn).unwrap().load(flags| PTEFlags::V);
    }
    pub fn load_all(&self) {
        for vpn in self.vpn_range {
            self.load_one(vpn);
        }
    }
    pub fn then_load_all(self) -> Self {
        self.load_all();
        self
    }

    pub fn fork(&self, other: &PageTable) -> Self {
        let new_pages = self.data_frames.iter().map(|(vpn, page)|(*vpn, page.fork(&mut other.create_force(*vpn)))).collect();
        Self::new(other,
            self.vpn_range.get_start(),
            self.vpn_range.get_end(),
            self.map_type,
            self.map_perm,
            self.file.clone(),
            self.file_offset,
            Some(new_pages)
        )
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
/// map type for memory set: identical or framed
pub enum MapType {
    Identity,
    Framed,
    /// Backed by a file
    #[allow(unused)]
    FileShared,
}

bitflags! {
    /// map permission corresponding to that in pte: `R W X U`
    pub struct MapPermission: u8 {
        ///Readable
        const R = 1 << 1;
        ///Writable
        const W = 1 << 2;
        ///Excutable
        const X = 1 << 3;
        ///Accessible in U mode
        const U = 1 << 4;
    }
}

impl From<MapPermission> for PTEFlags {
    fn from(value: MapPermission) -> Self {
        PTEFlags::from_bits(value.bits).unwrap()
    }
}

/// remap test in kernel space
#[allow(unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_SPACE.lock();
    let mid_text: VirtAddr = ((stext as usize + etext as usize) / 2).into();
    let mid_rodata: VirtAddr = ((srodata as usize + erodata as usize) / 2).into();
    let mid_data: VirtAddr = ((sdata as usize + edata as usize) / 2).into();
    assert!(!kernel_space
        .page_table
        .translate(mid_text.floor())
        .unwrap()
        .writable(),);
    assert!(!kernel_space
        .page_table
        .translate(mid_rodata.floor())
        .unwrap()
        .writable(),);
    assert!(!kernel_space
        .page_table
        .translate(mid_data.floor())
        .unwrap()
        .executable(),);
    println!("remap_test passed!");
}

/// size = 16, tolerable to strictly assign for each vpn a `Page` in `MapArea`.
enum Page {
    #[allow(unused)]
    Identity(MIdentityHandle),
    Framed(MFrameHandle),
    FileShared(MFileHandle),
}

unsafe impl Send for Page {}

impl Page {
    fn file_shared(pte: &mut PageTableEntry, inode: Arc<Inode>, offset: usize) -> Self {
        Self::FileShared(MFileHandle::map(pte as *mut PageTableEntry, inode, offset))
    }

    fn framed_lazy(pte: &mut PageTableEntry) -> Self {
        Self::Framed(MFrameHandle::map_lazy(pte as *mut PageTableEntry))
    }

    #[allow(unused)]
    fn framed_strict(pte: &mut PageTableEntry, flags: PTEFlags) -> Self {
        Self::Framed(MFrameHandle::map_strict(pte as *mut PageTableEntry, flags))
    }
    
    fn fork(&self, other: &mut PageTableEntry) -> Self {
        let other = other as *mut PageTableEntry;
        match self {
            Page::Identity(identity) => {
                let pte = unsafe { identity.pte.as_ref().unwrap() };
                let ppn = pte.ppn();
                let flags = pte.flags();
                Page::Identity(MIdentityHandle::map(
                    other,
                    VirtPageNum::from(ppn.0),
                    flags))
            }
            Page::Framed(framed) => {
                Page::Framed(framed.share_cow(other))
            }
            Page::FileShared(file_shared) => {
                Page::FileShared(file_shared.share_fully(other))
            }
        }
    }

    fn cown(&self) -> bool {
        match self {
            Page::Identity(_) => false,
            Page::FileShared(_) => false,
            Page::Framed(framed) => {
                if framed.is_cow() {
                    framed.cown();
                    true
                } else {
                    false
                }
            }
        }
    }

    fn load(&self, flags: PTEFlags) {
        match self {
            Page::Identity(_) => {}
            Page::Framed(frame_handle) => {
                frame_handle.load(flags);
            }
            Page::FileShared(file_handle) => {
                file_handle.load(flags);
            }
        }
    }
}

struct MIdentityHandle {
    pte: *mut PageTableEntry,
}

impl Drop for MIdentityHandle {
    fn drop(&mut self) {
        unsafe { self.pte.as_mut().unwrap().invalidate() };
    }
}

impl MIdentityHandle {
    fn map(pte: *mut PageTableEntry, vpn: VirtPageNum, flags: PTEFlags) -> Self {
        unsafe { *pte = PageTableEntry::new(PhysPageNum::from(vpn.0), flags) };
        Self {
            pte
        }
    }
}
