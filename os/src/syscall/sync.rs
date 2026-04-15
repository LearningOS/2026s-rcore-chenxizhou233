use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

const DEADLOCK: isize = -0xDEAD;

fn is_safe_state(available: &[usize], allocation: &[Vec<usize>], need: &[Vec<usize>]) -> bool {
    let mut work = available.to_vec();
    let mut finish = vec![false; allocation.len()];

    loop {
        let mut progress = false;
        for (tid, finished) in finish.iter_mut().enumerate() {
            if *finished {
                continue;
            }
            let can_finish = need[tid]
                .iter()
                .zip(work.iter())
                .all(|(required, available)| required <= available);
            if can_finish {
                for (work_one, alloc_one) in work.iter_mut().zip(allocation[tid].iter()) {
                    *work_one += *alloc_one;
                }
                *finished = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    finish.into_iter().all(|finished| finished)
}

fn request_safe(
    available: &[usize],
    allocation: &[Vec<usize>],
    need: &[Vec<usize>],
    tid: usize,
    rid: usize,
) -> bool {
    if tid >= allocation.len()
        || rid >= available.len()
        || need[tid][rid] == 0
        || available[rid] == 0
    {
        return false;
    }

    let mut test_available = available.to_vec();
    let mut test_allocation = allocation.to_vec();
    let mut test_need = need.to_vec();

    test_available[rid] -= 1;
    test_allocation[tid][rid] += 1;
    test_need[tid][rid] -= 1;

    is_safe_state(&test_available, &test_allocation, &test_need)
}

/// sleep syscall
pub fn sys_sleep(ms: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_sleep",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}
/// mutex create syscall
pub fn sys_mutex_create(blocking: bool) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
    let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;
        id as isize
    } else {
        process_inner.mutex_list.push(mutex);
        let ret = process_inner.mutex_list.len() as isize - 1;
        drop(process_inner);
        process.new_mutex_added();
        ret
    }
}
/// mutex lock syscall
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_lock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let rid = process_inner.mutex2rid[mutex_id];
    if process_inner.deadlock_detect_enabled
        && !request_safe(
            &process_inner.available,
            &process_inner.allocation,
            &process_inner.need,
            tid,
            rid,
        )
    {
        return DEADLOCK;
    }
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    drop(process);
    mutex.lock();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let rid = process_inner.mutex2rid[mutex_id];
    process_inner.available[rid] -= 1;
    process_inner.allocation[tid][rid] += 1;
    if process_inner.need[tid][rid] > 0 {
        process_inner.need[tid][rid] -= 1;
    }
    0
}
/// mutex unlock syscall
pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_mutex_unlock",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    drop(process);
    mutex.unlock();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let rid = process_inner.mutex2rid[mutex_id];
    process_inner.available[rid] += 1;
    process_inner.allocation[tid][rid] -= 1;
    0
}
/// semaphore create syscall
pub fn sys_semaphore_create(res_count: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));
        let ret = process_inner.semaphore_list.len() - 1;
        drop(process_inner);
        process.new_sem_added(res_count);
        ret
    };
    id as isize
}
/// semaphore up syscall
pub fn sys_semaphore_up(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_up",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    drop(process_inner);
    sem.up();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let rid = process_inner.sem2rid[sem_id];
    process_inner.available[rid] += 1;
    process_inner.allocation[tid][rid] -= 1;
    0
}
/// semaphore down syscall
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_semaphore_down",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let tid = current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid;
    let rid = process_inner.sem2rid[sem_id];
    if process_inner.deadlock_detect_enabled
        && !request_safe(
            &process_inner.available,
            &process_inner.allocation,
            &process_inner.need,
            tid,
            rid,
        )
    {
        return DEADLOCK;
    }
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    drop(process_inner);
    sem.down();
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let rid = process_inner.sem2rid[sem_id];
    process_inner.available[rid] -= 1;
    process_inner.allocation[tid][rid] += 1;
    if process_inner.need[tid][rid] > 0 {
        process_inner.need[tid][rid] -= 1;
    }
    0
}
/// condvar create syscall
pub fn sys_condvar_create() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}
/// condvar signal syscall
pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_signal",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}
/// condvar wait syscall
pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_condvar_wait",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}
/// enable deadlock detection syscall
///
/// YOUR JOB: Implement deadlock detection, but might not all in this syscall
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    trace!("kernel: sys_enable_deadlock_detect");
    if enabled != 0 && enabled != 1 {
        return -1;
    }

    let process = current_process();
    if enabled == 0 {
        process.inner_exclusive_access().deadlock_detect_enabled = false;
        return 0;
    }
    process.initialize();
    0
}
