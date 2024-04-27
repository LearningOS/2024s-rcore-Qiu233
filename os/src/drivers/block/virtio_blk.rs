use super::BlockDevice;
use crate::mm::{
    frame_alloc_contiguous, frame_dealloc, kernel_token, FrameTracker, PageTable, PhysAddr, PhysPageNum,
    StepByOne, VirtAddr,
};
use alloc::vec::Vec;
use lazy_static::*;
use spin::Mutex;
use virtio_drivers::{Hal, VirtIOBlk, VirtIOHeader};

/// The base address of control registers in Virtio_Block device
#[allow(unused)]
const VIRTIO0: usize = 0x10001000;
/// VirtIOBlock device driver strcuture for virtio_blk device
pub struct VirtIOBlock(Mutex<VirtIOBlk<'static, VirtioHal>>);

lazy_static! {
    static ref QUEUE_FRAMES: Mutex<Vec<FrameTracker>> = Mutex::new(Vec::new());
}

impl BlockDevice for VirtIOBlock {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        self.0
            .lock()
            .read_block(block_id, buf)
            .expect("Error when reading VirtIOBlk");
    }
    fn write_block(&self, block_id: usize, buf: &[u8]) {
        self.0
            .lock()
            .write_block(block_id, buf)
            .expect("Error when writing VirtIOBlk");
    }
}

impl VirtIOBlock {
    #[allow(unused)]
    /// Create a new VirtIOBlock driver with VIRTIO0 base_addr for virtio_blk device
    pub fn new() -> Self {
        unsafe {
            Self(Mutex::new(
                VirtIOBlk::<VirtioHal>::new(&mut *(VIRTIO0 as *mut VirtIOHeader)).unwrap(),
            ))
        }
    }
}

pub struct VirtioHal;

impl Hal for VirtioHal {
    fn dma_alloc(pages: usize) -> usize {
        // in multi-core environment, `frame_alloc` cannot be used for contiguous allocation
        let pages = frame_alloc_contiguous(pages).unwrap();
        let ppn_base = pages[0].ppn;
        QUEUE_FRAMES.lock().extend(pages.into_iter());
        let pa: PhysAddr = ppn_base.into();
        pa.0
    }

    fn dma_dealloc(pa: usize, pages: usize) -> i32 {
        let pa = PhysAddr::from(pa);
        let mut ppn_base: PhysPageNum = pa.into();
        for _ in 0..pages {
            frame_dealloc(ppn_base);
            ppn_base.step();
        }
        0
    }

    fn phys_to_virt(addr: usize) -> usize {
        addr
    }

    fn virt_to_phys(vaddr: usize) -> usize {
        PageTable::from_token(kernel_token())
            .translate_va(VirtAddr::from(vaddr))
            .unwrap()
            .0
    }
}
