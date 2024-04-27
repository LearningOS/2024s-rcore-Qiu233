use alloc::collections::BTreeMap;

use crate::{config::PAGE_SIZE, mm::address::StepByOne};
use super::{frame_alloc, FrameTracker, MMError, MMResult, PageError, PhysPageNum, VirtAddr, VirtPageNum};
use super::address::VPNRange;
use super::page_table::{PTEFlags, PageTable};

/// map area structure, controls a contiguous piece of virtual memory
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
}

impl MapArea {
    /// get vpn range
    pub fn get_range(&self) -> VPNRange {
        self.vpn_range
    }

    /// split the given area into two, with the same type and permission.<br/>
    /// `(self[start..vpn), self[vpn..end))` is returned
    pub fn split(self, vpn: VirtPageNum) -> (Self, Self) {
        let mut other = Self {vpn_range: VPNRange::new(vpn, vpn), data_frames: BTreeMap::new(), map_type: self.map_type, map_perm: self.map_perm};
        if vpn <= self.vpn_range.get_start() {
            return (other, self);
        } else if vpn >= self.vpn_range.get_end() {
            return (self, other);
        } else {
            let mut mapl = BTreeMap::new();
            let mut mapr = BTreeMap::new();
            // now collect `FrameTracker`s into different maps, according to their vpn
            for (i, frame) in self.data_frames.into_iter() { // self.data_frames moved here
                if i < vpn {
                    mapl.insert(i, frame);
                } else {
                    mapr.insert(i, frame);
                }
            }
            let left = Self {
                vpn_range: VPNRange::new(self.vpn_range.get_start(), vpn),
                data_frames: mapl,
                map_type: self.map_type,
                map_perm: self.map_perm
            };
            other = Self {
                vpn_range: VPNRange::new(vpn, self.vpn_range.get_end()),
                data_frames: mapr,
                map_type: self.map_type,
                map_perm: self.map_perm
            };
            return (left, other);
        }
    }
    /// create new map area
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
        }
    }

    /// Do not call this function directly
    fn ensure_page_raw(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) -> MMResult<()> {
        if !self.data_frames.contains_key(&vpn) {
            let frame = frame_alloc().ok_or(MMError::NotEnoughMemory)?;
            let ppn = frame.ppn;
            self.data_frames.insert(vpn, frame);
            let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
            match page_table.map(vpn, ppn, pte_flags) { // CRITICAL: drop the frame if mapping fails
                Ok(_) => return Ok(()),
                Err(e) => {
                    self.data_frames.remove(&vpn); // will be dropped
                    return Err(e);
                },
            }
        }
        Ok(())
    }
    /// ensure the intersection of the specified range
    #[allow(unused)]
    pub fn ensure_range(&mut self, page_table: &mut PageTable, vpn_range: VPNRange) -> MMResult<()> {
        match self.map_type {
            MapType::Identical => Ok(()),
            MapType::Framed => {
                let r = self.vpn_range.intersection(&vpn_range);
                for vpn in self.vpn_range.intersection(&vpn_range) {
                    self.ensure_page_raw(page_table, vpn)?;
                }
                Ok(())
            },
        }
    }
    /// ensure all virtual pages to be mapped
    pub fn ensure_all(&mut self, page_table: &mut PageTable) -> MMResult<()> {
        self.ensure_range(page_table, self.vpn_range)
    }

    /// For identity mappings, this function will map them all in page table.<br/>
    /// While for framed mappings, this function only emit an area but without actually allocating frames.
    fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) -> MMResult<()> {
        match self.map_type {
            MapType::Identical => {
                let ppn = PhysPageNum(vpn.0);
                let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
                page_table.map(vpn, ppn, pte_flags)
            }
            MapType::Framed => {
                Ok(())
            }
        }
    }

    /// unmap one virtual page, won't return error if the virtual page is not mapped
    #[allow(unused)]
    fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) -> MMResult<()> {
        if self.map_type == MapType::Framed {
            self.data_frames.remove(&vpn); // if there is mapped frame, then it's dropped
        }
        match page_table.unmap(vpn) {
            Ok(_) => Ok(()),
            Err(MMError::PageError(PageError::InvalidDirPage)) => Ok(()), // this error also indicates the frame is not prepared
            Err(MMError::PageError(PageError::PageInvalid)) => Ok(()),
            Err(e) => Err(e)
        }
    }
    /// Map the whole area, but without allocating frames stricly.
    pub fn map(&mut self, page_table: &mut PageTable) -> MMResult<()> {
        for vpn in self.vpn_range {
            match self.map_one(page_table, vpn) {
                Ok(_) => {},
                Err(e) => return Err(e)
            }
        }
        Ok(())
    }

    /// unmap the whole area
    pub fn unmap(&mut self, page_table: &mut PageTable) -> MMResult<()> {
        for vpn in self.vpn_range {
            match self.unmap_one(page_table, vpn) {
                Ok(_) => {},
                Err(e) => {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// shrink the area to a new end
    #[allow(unused)]
    pub fn shrink_to(&mut self, page_table: &mut PageTable, new_end: VirtPageNum) -> MMResult<()> {
        for vpn in VPNRange::new(new_end, self.vpn_range.get_end()) {
            match self.unmap_one(page_table, vpn) {
                Ok(_) => {},
                Err(e) => return Err(e)
            }
        }
        self.vpn_range = VPNRange::new(self.vpn_range.get_start(), new_end);
        Ok(())
    }

    /// expand the area to a new end
    #[allow(unused)]
    pub fn append_to(&mut self, page_table: &mut PageTable, new_end: VirtPageNum) -> MMResult<()> {
        for vpn in VPNRange::new(self.vpn_range.get_end(), new_end) {
            match self.map_one(page_table, vpn) {
                Ok(_) => {},
                Err(e) => return Err(e)
            }
        }
        self.vpn_range = VPNRange::new(self.vpn_range.get_start(), new_end);
        Ok(())
    }
    /// data: start-aligned but maybe with shorter length.<br/>
    /// assume that all frames were cleared before.<br/>
    /// This function will ensure that required frames are allocated before actually copying data.
    pub fn copy_data(&mut self, page_table: &mut PageTable, data: &[u8]) -> MMResult<()> {
        assert_eq!(self.map_type, MapType::Framed);
        let pages = (data.len() - 1 + PAGE_SIZE) / PAGE_SIZE;
        assert!(pages <= self.vpn_range.into_iter().count()); // data's length cannot exceed the area size
        self.ensure_range(page_table, VPNRange::by_len(self.vpn_range.get_start(), pages))?;
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)?
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
        Ok(())
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
/// map type for memory set: identical or framed
pub enum MapType {
    /// identity mappings
    Identical,
    /// framed mappings
    Framed,
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
