//! Implementation of [`MapArea`] and [`MemorySet`].

use super::{MapArea, MapPermission, MapType};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, VirtAddr, VirtPageNum};
use super::VPNRange;
use super::err::{AreaError, MMResult};
use crate::config::{
    KERNEL_STACK_SIZE, MEMORY_END, PAGE_SIZE, TRAMPOLINE, TRAP_CONTEXT_BASE, USER_STACK_SIZE,
};
use crate::sync::UPSafeCell;
use alloc::sync::Arc;
use alloc::vec::Vec;
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
    pub static ref KERNEL_SPACE: Arc<UPSafeCell<MemorySet>> =
        Arc::new(unsafe { UPSafeCell::new(MemorySet::new_kernel()) });
}
/// address space
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}

impl MemorySet {
    /// Create a new empty `MemorySet`.
    pub fn new_bare() -> MMResult<Self> {
        let pt = PageTable::new()?;
        Ok(Self {
            page_table: pt,
            areas: Vec::new(),
        })
    }
    /// Get the page table token
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// Insert framed area strictly
    pub fn insert_framed_area_strict(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) -> MMResult<()> {
        self.push_strict(
            MapArea::new(start_va, end_va, MapType::Framed, permission),
            None,
        )
    }
    /// Insert framed area lazily
    pub fn insert_framed_area_lazy(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) -> MMResult<()> {
        self.push_lazy(
            MapArea::new(start_va, end_va, MapType::Framed, permission),
            None,
        )
    }
    /// Push an area and **strictly** allocate frames for it
    fn push_strict(&mut self, mut map_area: MapArea, data: Option<&[u8]>) -> MMResult<()> {
        map_area.map(&mut self.page_table)?;
        map_area.ensure_all(&mut self.page_table)?; // force allocation
        if let Some(data) = data {
            map_area.copy_data(&mut self.page_table, data)?;
        }
        // if the above operations fails, then `map_area` is not kept and thus dropped, 
        // in which case, all partially alllocated frames are collected again.
        self.areas.push(map_area);
        Ok(())
    }
    /// Push an area lazily.<br/>
    /// Frames will be partially allocated if data is not `None`.<br/>
    fn push_lazy(&mut self, mut map_area: MapArea, data: Option<&[u8]>) -> MMResult<()> {
        map_area.map(&mut self.page_table)?;
        if let Some(data) = data {
            // `copy_data` will in turn call `translate` which ensures the requested page is prepared
            map_area.copy_data(&mut self.page_table, data)?;
        }
        // if the above operations fails, then `map_area` is not kept and thus dropped, 
        // in which case, all partially alllocated frames are collected again.
        self.areas.push(map_area);
        Ok(())
    }
    /// Mention that trampoline is not collected by areas.
    fn map_trampoline(&mut self) -> MMResult<()> {
        self.page_table.map(
            VirtAddr::from(TRAMPOLINE).into(),
            PhysAddr::from(strampoline as usize).into(),
            PTEFlags::R | PTEFlags::X,
        )
    }
    /// Without kernel stacks.
    pub fn new_kernel() -> Self {
        let memory_set = Self::new_bare();
        // this cannot fail, as there's only one kernel space initialized on startup.
        // if fails, the kernel should be slimmed.
        assert!(memory_set.is_ok(), "failed to allocate kernel space, err = {}", memory_set.err().unwrap());
        let mut memory_set = memory_set.unwrap();
        // map trampoline
        memory_set.map_trampoline().unwrap();
        // map kernel sections
        info!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        info!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        info!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        info!(
            ".bss [{:#x}, {:#x})",
            sbss_with_stack as usize, ebss as usize
        );
        info!("mapping .text section");
        memory_set.push_lazy(
            MapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
            ),
            None,
        ).unwrap();
        info!("mapping .rodata section");
        memory_set.push_lazy(
            MapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
            ),
            None,
        ).unwrap();
        info!("mapping .data section");
        memory_set.push_lazy(
            MapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        ).unwrap();
        info!("mapping .bss section");
        memory_set.push_lazy(
            MapArea::new(
                (sbss_with_stack as usize).into(),
                (ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        ).unwrap();
        info!("mapping physical memory");
        memory_set.push_lazy(
            MapArea::new(
                (ekernel as usize).into(),
                MEMORY_END.into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        ).unwrap();
        memory_set
    }
    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp_base and entry point.
    pub fn from_elf(elf_data: &[u8]) -> MMResult<(Self, usize, usize)> {
        let mut memory_set = Self::new_bare()?;
        // map trampoline
        memory_set.map_trampoline()?;
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
                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_end_vpn = map_area.get_range().get_end();
                // loaded area should always be strict, as they don't require more than needed,
                // and for now we have no way for lazy load.
                memory_set.push_strict(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                )?;
            }
        }
        // map user stack with U flags
        let max_end_va: VirtAddr = max_end_vpn.into();
        let mut user_stack_bottom: usize = max_end_va.into();
        // guard page
        user_stack_bottom += PAGE_SIZE;
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        memory_set.push_lazy(
            MapArea::new(
                user_stack_bottom.into(),
                user_stack_top.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
            None,
        )?;
        // used in sbrk
        memory_set.push_lazy(
            MapArea::new(
                user_stack_top.into(),
                user_stack_top.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
            None,
        )?;
        // map TrapContext
        memory_set.push_strict( // CRITICAL: this must be strict so trap handling works normally
            MapArea::new(
                TRAP_CONTEXT_BASE.into(),
                TRAMPOLINE.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W,
            ),
            None,
        )?;
        Ok((
            memory_set,
            user_stack_top,
            elf.header.pt2.entry_point() as usize,
        ))
    }
    /// Change page table by writing satp CSR Register.
    pub fn activate(&self) {
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp);
            asm!("sfence.vma");
        }
    }
    #[allow(unused)]
    fn find_area_ensure(&mut self, vpn: VirtPageNum) -> MMResult<()> {
        if let Some(area) = self.areas.iter_mut().find(|x|x.get_range().contains(&vpn)) {
            area.ensure_range(&mut self.page_table, VPNRange::by_len(vpn, 1))
        } else {
            Err(AreaError::AreaRangeNotInclude.into())
        }
    }
    /// Translate a virtual page number to a page table entry.<br/>
    /// Calling this function will forcibly allocate a frame for the requested page.
    pub fn translate(&mut self, vpn: VirtPageNum) -> MMResult<PageTableEntry> {
        self.find_area_ensure(vpn)?;
        self.page_table.translate(vpn)
    }
    /// shrink the area to new_end
    #[allow(unused)]
    pub fn shrink_to(&mut self, start: VirtAddr, new_end: VirtAddr) -> MMResult<()> {
        if let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.get_range().get_start() == start.floor())
        {
            area.shrink_to(&mut self.page_table, new_end.ceil())
        } else {
            Err(AreaError::NoMatchingArea.into())
        }
    }

    /// append the area to new_end
    #[allow(unused)]
    pub fn append_to(&mut self, start: VirtAddr, new_end: VirtAddr) -> MMResult<()> {
        if let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.get_range().get_start() == start.floor())
        {
            area.append_to(&mut self.page_table, new_end.ceil())
        } else {
            Err(AreaError::NoMatchingArea.into())
        }
    }

    /// test if there are mapped area whitin the given range.<br/>
    /// note that this doesn't check mappings which are not tracked by `MapArea`s
    fn has_mapped(&self, range: VPNRange) -> bool {
        self.areas.iter().any(|x|x.get_range().intersects(&range))
    }

    /// test if there are unmapped area whitin the given range.<br/>
    /// note that this doesn't check mappings which are not tracked by `MapArea`s
    fn has_unmapped(&self, range: VPNRange) -> bool {
        let count = self.areas.iter().map(|x|{
            let (_, _, rem) = x.get_range().exclude(&range);
            rem.into_iter().count()
        }).sum::<usize>();
        
        let expected = range.into_iter().count();
        count != expected
    }

    /// Gets whether the specified virtual page is critical and thus cannot be unmapped.
    fn is_critical(&self, vpn: VirtPageNum) -> bool {
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
    ) -> MMResult<()>  {
        let area = MapArea::new(start_va, end_va, MapType::Framed, permission);
        if area.get_range().into_iter().any(|x|self.is_critical(x)) {
            return Err(AreaError::AreaCritical.into());
        }
        if self.has_mapped(area.get_range()) {
            return Err(AreaError::AreaHasMappedPortion.into());
        }
        self.push_lazy(
            area,
            None,
        )
    }

    /// Try to unmap virtual address range, except for **critical mappings** such as `TRAMPOLINE` and `TRAP_CONTEXT_BASE`.
    /// One area will be split into two if it's unmapped in the middle.
    pub fn munmap(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
    ) -> MMResult<()>  {
        let target_range = VPNRange::new(start_va.floor(), end_va.ceil());
        if target_range.into_iter().any(|x|self.is_critical(x)) {
            return Err(AreaError::AreaCritical.into());
        }
        if self.has_unmapped(target_range) {
            return Err(AreaError::AreaHasUnmappedPortion.into());
        }
        let areas = core::mem::take(&mut self.areas);
        for area in areas.into_iter() {
            // compute ranges
            let (l, _, rem) = area.get_range().exclude(&target_range);
            if rem.is_empty() { // nothing to remove in this area, push and skip
                self.areas.push(area);
                continue;
            }
            let (larea, rarea) = area.split(l.get_end());
            let (mut marea, rarea) = rarea.split(rem.get_end());
            // now `larea`/`rarea` are the left/right parts to preserve, respectively
            // if some of them are empty, then there's no need to push back
            if !larea.get_range().is_empty() {
                self.areas.push(larea);
            }
            if !rarea.get_range().is_empty() {
                self.areas.push(rarea);
            }
            marea.unmap(&mut self.page_table)?;
            drop(marea); // this can be omitted, but I choose to make it clear that `marea` is collected
        }
        Ok(())
    }

}

/// Return (bottom, top) of a kernel stack in kernel space.
pub fn kernel_stack_position(app_id: usize) -> (usize, usize) {
    let top = TRAMPOLINE - app_id * (KERNEL_STACK_SIZE + PAGE_SIZE);
    let bottom = top - KERNEL_STACK_SIZE;
    (bottom, top)
}

/// remap test in kernel space
#[allow(unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_SPACE.exclusive_access();
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
