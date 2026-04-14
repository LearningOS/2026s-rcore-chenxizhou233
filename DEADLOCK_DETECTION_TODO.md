# rCore 死锁检测实现 TODO

## 概述
本文档根据 Banker's Algorithm（银行家算法）为 rCore 死锁检测功能列出详细的实现步骤。

## 已完成的框架工作
- ✅ [os/src/task/process.rs](os/src/task/process.rs)：添加 `deadlock_detect_enabled`, `available`, `allocation`, `need` 字段
- ✅ 进程初始化时初始化这些字段为空/禁用状态
- ✅ [os/src/syscall/sync.rs](os/src/syscall/sync.rs)：在 sys_mutex_lock、sys_semaphore_down、sys_enable_deadlock_detect 中添加 TODO 注释

## 实现阶段

### 第一阶段：理解数据结构映射

**资源在 rCore 中的含义：**
- Mutex：1 份资源（要么持有，要么不持有）
- Semaphore：多份资源（可用许可证的个数）

**数据结构映射：**
```
available[i]     = 第 i 个资源当前还有多少个可用
                 i < mutex_count: 1 表示可用，0 表示被持有
                 i >= mutex_count: semaphore[i - mutex_count] 的剩余计数

allocation[tid][i] = 线程 tid 当前持有第 i 个资源的个数

need[tid][i]     = 线程 tid 还需要多少个第 i 个资源
                 （只有在请求时才增加，获取后减少）
```

### 第二阶段：sys_enable_deadlock_detect 实现

**框架 6 位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) 第 306 行

**任务：**
1. ☐ 读取参数 `enabled`，1 表示启用，0 表示禁用
2. ☐ 设置 `process_inner.deadlock_detect_enabled = (enabled != 0)`
3. ☐ **首次启用时**，初始化三个表：
   - `available` 大小 = `mutex_list.len() + semaphore_list.len()`
     - 前 `mutex_list.len()` 个元素初始为 1（每个 mutex 有 1 份资源）
     - 后面的元素初始为各 semaphore 的初始计数值
       ```rust
       // 伪代码
       for (i, sem) in semaphore_list.iter().enumerate() {
           if let Some(s) = sem {
               available[mutex_list.len() + i] = s.get_count();  // 需要实现获取方法
           }
       }
       ```
   
   - `allocation` 大小 = `tasks.len() × (mutex_count + semaphore_count)`，全初始为 0
   - `need` 大小 = `tasks.len() × (mutex_count + semaphore_count)`，全初始为 0

4. ☐ 返回 0 表示成功，非 0 表示失败

### 第三阶段：sys_mutex_lock 改动

**框架 4 位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) ~第 72 行

**当前状态：** 代码已加上"获取—直接 lock"前的注释位置

**任务：**
1. ☐ 在 `mutex.lock()` 前，检查死锁检测是否启用：
   ```rust
   if process_inner.deadlock_detect_enabled {
       // 获取当前线程 tid 和 mutex 对应的 resource_id
       let tid = current_task().unwrap().inner_exclusive_access().res.as_ref().unwrap().tid;
       let resource_id = mutex_id;  // mutex 的 resource_id 就是它在列表中的位置
       
       // 临时更新 need：表示这个线程即将请求这个资源
       process_inner.need[tid][resource_id] += 1;
       
       // 调用检测函数
       if deadlock_detect(&mut process_inner, tid, resource_id) < 0 {
           // 检测到不安全，恢复并返回
           process_inner.need[tid][resource_id] -= 1;
           return -0xDEAD;  // -3757
       }
   }
   ```

2. ☐ 成功通过检测后，正常执行 `mutex.lock()`

3. ☐ 获取成功后，更新资源表：
   ```rust
   process_inner.allocation[tid][mutex_id] += 1;
   process_inner.need[tid][mutex_id] -= 1;
   process_inner.available[mutex_id] = 0;  // mutex 被持有
   ```

### 第四阶段：sys_semaphore_down 改动

**框架 5 位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) ~第 164 行

**任务：** 同 sys_mutex_lock，但注意：
1. ☐ `resource_id = mutex_list.len() + sem_id`（因为 semaphore 的 id 空间是分开的）
2. ☐ 检测到不安全时，直接返回 -0xDEAD，不执行 `sem.down()`
3. ☐ 成功 down 后，更新表类似 mutex，但需要注意 available 的减少是 -1，而不是设为 0

### 第五阶段：sys_mutex_unlock 改动

**位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) ~第 95 行

**任务：**
1. ☐ 在 `mutex.unlock()` 后，更新资源表：
   ```rust
   if process_inner.deadlock_detect_enabled {
       let tid = current_task().unwrap().inner_exclusive_access().res.as_ref().unwrap().tid;
       process_inner.allocation[tid][mutex_id] -= 1;
       process_inner.available[mutex_id] = 1;  // 恢复为可用
   }
   ```

### 第六阶段：sys_semaphore_up 改动

**位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) ~第 148 行

**任务：**
1. ☐ 在 `sem.up()` 后，更新资源表：
   ```rust
   if process_inner.deadlock_detect_enabled {
       let tid = current_task().unwrap().inner_exclusive_access().res.as_ref().unwrap().tid;
       let resource_id = mutex_list.len() + sem_id;
       process_inner.allocation[tid][resource_id] -= 1;
       process_inner.available[resource_id] += 1;  // semaphore 计数 +1
   }
   ```

### 第七阶段：sys_mutex_create 改动

**位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) ~第 37 行（函数末尾返回前）

**任务：**
1. ☐ 每次创建 mutex 时，扩展 available、allocation、need 的规模
   ```rust
   if process_inner.deadlock_detect_enabled {
       // 新增一列表示新的 mutex
       process_inner.available.push(1);  // 新 mutex 可用
       for row in process_inner.allocation.iter_mut() {
           row.push(0);
       }
       for row in process_inner.need.iter_mut() {
           row.push(0);
       }
   }
   ```

### 第八阶段：sys_semaphore_create 改动

**位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) ~第 131 行（函数末尾返回前）

**任务：**
1. ☐ 每次创建 semaphore 时，扩展 available、allocation、need 的规模
   ```rust
   if process_inner.deadlock_detect_enabled {
       // 新增一列表示新的 semaphore
       process_inner.available.push(res_count);  // 初值为 res_count
       for row in process_inner.allocation.iter_mut() {
           row.push(0);
       }
       for row in process_inner.need.iter_mut() {
           row.push(0);
       }
   }
   ```

### 第九阶段：deadlock_detect 函数实现（核心算法）

**框架 3 位置：** [os/src/syscall/sync.rs](os/src/syscall/sync.rs) 末尾

**注意：** 此函数应该定义在 [os/src/task/process.rs](os/src/task/process.rs) 中作为函数而不是方法（因为要避免私有字段访问问题）

**算法伪代码：**
```
function deadlock_detect(available, allocation, need, tid, req_resource):
    // Step 1: 临时增加当前线程的 need
    need[tid][req_resource] += 1
    
    // Step 2: 初始化 Work = available 的副本，Finish 全为 false
    work = available.clone()
    finish = vec![false; num_threads]
    
    // Step 3: 反复寻找可以继续的线程
    loop:
        found = false
        for j in 0..num_threads:
            if not finish[j] and need[j] <= work:  // 所有资源都够
                finish[j] = true
                work += allocation[j]  // 释放线程 j 的资源
                found = true
                break  // 重新开始循环
        
        if not found:
            break  // 没有找到可继续的线程，退出循环
    
    // Step 4: 恢复该线程的 need
    need[tid][req_resource] -= 1
    
    // Step 5: 检查结果
    if all finish[i] == true:
        return 0  // 安全
    else:
        return -0xDEAD  // 不安全，存在死锁可能
```

**Rust 实现框架：**
```rust
fn deadlock_detect(
    available: &[usize],
    allocation: &[Vec<usize>],
    need: &mut [Vec<usize>],
    tid: usize,
    resource_id: usize,
) -> isize {
    // TODO: 填充实现
    // 1. 创建 work 向量
    // 2. 创建 finish 向量
    // 3. 临时增加 need[tid][resource_id]
    // 4. 核心算法：循环寻找可继续的线程
    // 5. 恢复 need[tid][resource_id]
    // 6. 检查所有线程是否都完成，返回 0 或 -0xDEAD
    0
}
```

## 测试建议

1. ☐ 编写简单的无死锁场景：多个线程按相同顺序获取多个 mutex，验证都能获取成功
2. ☐ 编写死锁场景：两个线程分别按不同顺序获取两个 mutex，验证第二个请求被拒绝或返回 -0xDEAD
3. ☐ 验证 semaphore 的情况
4. ☐ 验证禁用检测时不拒绝任何请求

## 关键细节

- **需要的辅助方法：** 
  - Semaphore 需要提供 `get_count()` 方法以获取当前计数
  - ProcessControlBlockInner 可能需要提供快速访问 resource_id → (resource_type, index) 的映射函数

- **性能考虑：**
  - deadlock_detect 函数可能会被频繁调用，需要优化
  - 可以考虑只在特定条件下启用检测以减少开销

- **边界情况：**
  - 进程只有一个线程时
  - 没有创建任何 mutex/semaphore 时
  - 线程退出时的资源清理

## 参考文献

图片中的算法描述已经是完整的 Banker's Algorithm 实现指南，按照图片中的步骤 1-4 逐个翻译为代码即可。
