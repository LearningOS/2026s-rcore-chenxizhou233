use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;

// ========== 死锁检测完整实现指南 ==========
//
// 实现步骤总览：
//
// 第一阶段：数据结构初始化（已部分完成）
// ✓ 框架初始化 1、2：ProcessControlBlockInner 里加入 available/allocation/need 字段
// ✓ 框架初始化 3、4：进程 new() 和 fork() 时初始化这些字段
//
// 第二阶段：系统调用改动
// □ 框架 4（sys_mutex_lock）
//   - 在实际 lock 前调用 deadlock_detect()
//   - 如果返回 -0xDEAD，直接返回错误，不获取锁
//   - 如果安全（返回 0），正常获取锁后更新 Allocation 和 Need
//
// □ 框架 5（sys_semaphore_down）
//   - 同 mutex_lock，但 resource_id = mutex_list.len() + sem_id
//   - 调用 deadlock_detect() 检测
//   - 如果安全，执行 down() 后更新 Allocation 和 Need
//
// □ 框架 6（sys_enable_deadlock_detect）
//   - 启用时初始化 available/allocation/need 表
//   - 禁用时可以保持或清空表
//
// □ 其他需要改动的地方（没有框架，但有道理）：
//   - sys_mutex_create：创建时需要扩展 available、allocation、need 的规模
//   - sys_semaphore_create：同上，且初始 available[resource_id] = 初值
//   - sys_mutex_unlock：释放时 Allocation[tid][mutex_id] -= 1
//   - sys_semaphore_up：释放时 Allocation[tid][resource_id] -= 1
//
// 第三阶段：检测算法实现
// □ 框架 3（deadlock_detect 函数）
//   - 实现图片里的 Banker's Algorithm
//   - 临时增加当前线程的 Need，做安全检测，再恢复
//   - 通过模拟所有线程的完成顺序来判断是否安全
//

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
        process.new_mutex_added();
        process_inner.mutex_list.len() as isize - 1
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
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());

    // TODO (框架 4)：死锁检测入点-MUTEX_LOCK
    // 1. 先检查死锁检测是否启用：if process_inner.deadlock_detect_enabled
    // 2. 如果启用，更新 Need 表：当前线程对 mutex_id 的需求 +1
    // 3. 调用 deadlock_detect() 检查系统是否会进入不安全状态
    // 4. 如果检测返回不安全（-0xDEAD），就直接返回 -0xDEAD，不获取锁
    // 5. 如果安全或检测禁用，继续正常逻辑：获取锁后，Allocation[tid][mutex_id] += 1，Need[tid][mutex_id] -= 1
    if process_inner.deadlock_detect_enabled {}

    drop(process_inner);
    drop(process);
    mutex.lock();
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
        process.new_sem_added(res_count);
        process_inner.semaphore_list.len() - 1
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
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());

    // TODO (框架 5)：死锁检测入点-SEMAPHORE_DOWN
    // 1. 先检查死锁检测是否启用：if process_inner.deadlock_detect_enabled
    // 2. 如果启用，更新 Need 表：当前线程对 sem_id 的需求 +1
    //    （注意 resource_id = mutex_list.len() + sem_id，因为 Available/Allocation/Need 是统一的向量）
    // 3. 调用 deadlock_detect() 检查系统是否会进入不安全状态
    // 4. 如果检测返回不安全（-0xDEAD），就直接返回 -0xDEAD，不执行 down
    // 5. 如果安全或检测禁用，继续正常逻辑：执行 down 后，Allocation[tid][resource_id] += 1，Need[tid][resource_id] -= 1
    drop(process_inner);
    sem.down();
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
    let _process_inner = process.inner_exclusive_access();

    // TODO (框架 6)：启用/禁用死锁检测
    // 1. 设置 process_inner.deadlock_detect_enabled = (enabled != 0)
    // 2. 如果第一次启用（从 false 变为 true），需要初始化 available、allocation、need 表
    //    - available 大小 = mutex_list.len() + semaphore_list.len()
    //    - 前 mutex_list.len() 个元素初始化为 1（每个 mutex 只有 1 份资源）
    //    - 后面的元素初始化为每个 semaphore 的初始计数
    //    - allocation 和 need 的大小 = tasks.len() × (mutex_count + semaphore_count)
    //    - 初始 allocation 全为 0（线程还没有任何资源）
    //    - 初始 need 也全为 0
    // 3. 如果禁用（从 true 变为 false），可以保持表内容或清空（取决于你的设计）
    // 4. 返回 0 表示成功

    0
}

// deadlock_detect 函数框架已移至 process.rs 中的 ProcessControlBlock 实现
