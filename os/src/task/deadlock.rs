//! deadlock detection

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::vec;

use crate::sync::{Mutex, Semaphore};


pub struct DeadlockDetection<T: LockRelease> {
    pub locks: Vec<Option<T>>,
    pub alloc: Vec<Vec<usize>>,
    pub q: Vec<Vec<usize>>,
    pub avail: Vec<usize>,
}

pub trait LockRelease {
    fn up(&self);
}


impl<T: LockRelease> DeadlockDetection<T> {
    pub fn new(locks: Vec<Option<T>>) -> Self {
        Self {
            locks,
            alloc: Vec::new(),
            q: Vec::new(),
            avail: Vec::new(),
        }
    }

    fn all_zero(v: &Vec<usize>) -> bool {
        v.iter().all(|x|*x == 0)
    }

    fn le_vec(v1: &Vec<usize>, v2: &Vec<usize>) -> bool {
        assert!(v1.len() == v2.len());
        v1.iter().zip(v2.iter()).all(|(x,y)|*x<=*y)
    }

    fn add_to(dst: &mut Vec<usize>, src: &Vec<usize>) {
        assert!(dst.len() == src.len());
        dst.iter_mut().zip(src.iter()).for_each(|(x,y)|*x += *y);
    }

    pub fn prepare_lock_state(&mut self, num_tid: usize, num_lock: usize) {
        while self.alloc.len() < num_tid {
            self.alloc.push(Vec::new());
        }
        while self.q.len() < num_tid {
            self.q.push(Vec::new());
        }
        for i in 0..num_tid {
            while self.alloc[i].len() < num_lock {
                self.alloc[i].push(0);
            }
            while self.q[i].len() < num_lock {
                self.q[i].push(0);
            }
        }
    }

    pub fn pre_wait(&mut self, tid: usize, lock_id: usize) {
        self.q[tid][lock_id] += 1;
    }
    pub fn post_wait_succ(&mut self, tid: usize, lock_id: usize) {
        assert!(self.avail[lock_id] > 0);
        self.q[tid][lock_id] -= 1;
        self.avail[lock_id] -= 1;
        self.alloc[tid][lock_id] += 1;
    }
    pub fn release_all_locks(&mut self, tid: usize) {
        Self::add_to(&mut self.avail, &self.alloc[tid]);
        self.alloc[tid].iter_mut().enumerate().for_each(|(lock_id, x)|{
            while *x > 0 {
                self.locks[lock_id].as_ref().unwrap().up();
                *x -= 1;
            }
        });
    }
    pub fn post_wait_fail(&mut self, tid: usize, lock_id: usize) {
        assert!(self.q[tid][lock_id] > 0);
        self.q[tid][lock_id] -= 1;
        self.release_all_locks(tid);
    }
    pub fn pre_release(&mut self, tid: usize, lock_id: usize) {
        self.prepare_lock_state(tid + 1, lock_id + 1);
        assert!(self.alloc[tid][lock_id] > 0);
    }
    pub fn post_release(&mut self, tid: usize, lock_id: usize) {
        self.avail[lock_id] += 1;
        self.alloc[tid][lock_id] -= 1;
    }

    /// detect deadlock by given environment
    pub fn has_deadlock(&mut self) -> bool {
        let num_threads = self.alloc.len(); // `prepare_lock_state` will extend it to needed 
        let num_lock = self.locks.len();
        self.prepare_lock_state(num_threads, num_lock);
        let q = &mut self.q;
        let alloc = &mut self.alloc;
        let avail = &self.avail;
        let mut finish = vec![false; num_threads];
        for i in 0..num_threads {
            if Self::all_zero(&alloc[i]) {
                finish[i] = true;
            }
        }
        #[allow(non_snake_case)]
        let mut W = avail.clone();
        loop {
            let mut found = None;
            for i in 0..num_threads {
                let cond1 = !finish[i];
                let cond2 = Self::le_vec(&q[i], &W);
                if cond1 && cond2 {
                    finish[i] = true;
                    Self::add_to(&mut W, &alloc[i]);
                    found = Some(i);
                    break;
                }
            }
            if found.is_none() {
                break;
            }
        }
        let deadlock = !finish.iter().all(|x|*x);
        deadlock
    }
    

    pub fn has_deadlock_if_wait_for(&mut self, tid: usize, lock_id: usize) -> bool {
        self.prepare_lock_state(tid + 1, lock_id + 1);
        self.q[tid][lock_id] += 1;
        let result = self.has_deadlock();
        self.q[tid][lock_id] -= 1;
        result
    }
}

impl LockRelease for Arc<Semaphore> {
    fn up(&self) {
        Semaphore::up(&self);
    }
}


impl LockRelease for Arc<dyn Mutex> {
    fn up(&self) {
        self.unlock();
    }
}
