
pub use self::hart_local::{get_hartid, get_processor};
pub use self::hart_init::init_harts;

mod hart_init {
    use core::arch::global_asm;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use riscv::register::{satp, sstatus};
    use crate::task::{kstack_alloc, run_tasks};

    global_asm!(include_str!("hart.asm"));

    /// Initialize all harts, including the invoking one which is also the booting one.
    /// 
    /// Description.
    /// * `post_init` - action to run after the boot hart initialization, typically used to prepare the first process.
    pub fn init_harts(post_init: fn()) {
        extern "C" {
            fn _hart_start();
            fn BOOT_HART();
        }
        unsafe {
            let satp = satp::read().bits();
            let boot_hart = *(BOOT_HART as usize as *const usize);
            init_hart(boot_hart); // init boot hart
            get_harts().filter(|x|*x != boot_hart).for_each(|x|start_hart(x, satp));
            post_init(); // call it before entering scheduling loop
            crate::task::run_tasks(); // run the invoking hart
        }
    }
    /// should be called for every hart including the boot hart
    fn init_hart(hartid: usize) {
        super::hart_local::init_hart_local_data(hartid);
        unsafe { sstatus::set_spie() }; // enable interrupt after entering user mode
        crate::trap::init();
        crate::trap::enable_timer_interrupt();
        crate::timer::set_next_trigger();
    }
    fn get_harts() -> impl Iterator<Item = usize> {
        extern "C" {
            fn DTB_POINTER();
        }
        unsafe {
            let addr = *(DTB_POINTER as usize as *const usize);
            trace!("DTB address = {:#x}", addr);
            let dtb = hermit_dtb::Dtb::from_raw(addr as *const u8).expect("failed to parse dtb");
            dtb.enum_subnodes("cpus")
                .filter(|x|x.contains("cpu@"))
                .filter_map(|x|x.split("@").nth(1))
                .filter_map(|x|x.parse::<usize>().ok())
        }
    }
    
    /// `sp` and `satp` are set before entering this function
    #[no_mangle]
    fn hart_main(hartid: usize, sig: &AtomicUsize) -> ! {
        trace!("current hartid = {}", hartid);
        while sig.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {}
        // starting from here, `sig` is no longer available
    
        init_hart(hartid);
        run_tasks();
        panic!("Unreachable in hart_main!");
    }
    
    /// Only used when starting hart.
    #[repr(C)]
    struct HartInitContext {
        sp: usize,
        satp: usize,
        sig: AtomicUsize
    }
    
    fn start_hart(id: usize, satp: usize) {
        extern "C" {
            fn _hart_start();
        }
        let stack = kstack_alloc(); // these allocation will be permanent
        let hart_ctx = HartInitContext {
            sp: stack.get_top(),
            satp,
            sig: AtomicUsize::new(0)
        };
        let result = crate::sbi::hart_start(id, _hart_start as usize, &hart_ctx as *const HartInitContext as usize);
        assert!(result == 0, "failed to start hart id = {}, result = {}", id, result);
        // must wait until the hart has been initialized
        while hart_ctx.sig.compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst).is_err() {}
        core::mem::forget(stack); // hart stack cannot be dropped
        trace!("successfully initialized hart #{}", id);
    }
}

mod hart_local {
    use core::arch::asm;
    use alloc::boxed::Box;
    use spin::{Mutex, MutexGuard};
    use crate::task::Processor;

    /// Hart local data storage.<br/>
    /// This is allocated on the kernel heap.
    pub struct HartLocalData {
        hartid: usize,
        processor: Mutex<Processor>
    }


    /// get hart id for the current hart
    #[allow(unused)]
    #[inline]
    pub fn get_hartid() -> usize {
        get_hart_local_data().hartid
    }


    /// get processor for the current hart
    #[inline]
    pub fn get_processor() -> MutexGuard<'static, Processor> {
        get_hart_local_data().processor.lock()
    }
    
    pub fn init_hart_local_data(hartid: usize) {
        unsafe {
            // permanent allocation on heap for storing hart-local data
            let processor = Mutex::new(Processor::new());
            let data = HartLocalData{
                hartid,
                processor
            };
            let tp = Box::into_raw(Box::new(data)) as usize;
            asm!("mv tp, {}", in(reg) tp);
        }
    }

    #[inline]
    fn get_hart_local_data() -> &'static mut HartLocalData {
        unsafe {
            let mut tp: usize;
            asm!("mv {}, tp", out(reg) tp);
            (tp as *mut HartLocalData).as_mut().unwrap()
        }
    }

}