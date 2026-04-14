# Deadlock Detection 实现总结

## 概述
实现了基于 Banker 算法的死锁检测机制，用于 mutex 和 semaphore 的安全锁定操作。当死锁检测启用时，`sys_mutex_lock` 和 `sys_semaphore_down` 会在操作前检查系统安全性，若不安全则返回 `-0xDEAD` (-3725)。

## 涉及文件

### 1. `os/src/task/process.rs` - 核心死锁检测逻辑

#### 添加字段（ProcessControlBlockInner）
```rust
// 死锁检测相关字段
pub deadlock_detect_enabled: bool,           // 是否启用死锁检测
pub available: Vec<u32>,                      // 可用资源计数
pub allocation: Vec<Vec<u32>>,               // allocation[tid][rid] 已分配
pub need: Vec<Vec<u32>>,                     // need[tid][rid] 需要量
pub mutex_id_to_index: HashMap<u32, usize>, // mutex_id 到资源索引
pub semaphore_id_to_index: HashMap<u32, usize>, // sem_id 到资源索引
```

#### 核心方法

**1. `deadlock_enabled() -> bool`**
- 返回是否启用了死锁检测

**2. `deadlock_init_state()`**
- 初始化可用资源计数
- 统计当前 mutex 和 semaphore 的总数
- 初始化 allocation 和 need 为零矩阵

**3. `ensure_deadlock_shape()`**
- 动态扩展矩阵以容纳新线程和新资源
- 当创建新的 mutex/semaphore 时扩展列
- 当创建新线程时扩展行

**4. `deadlock_safe_check() -> bool`**
- 实现 Banker 算法的安全性检查
- 使用 Work 向量和 Finish 数组模拟资源分配
- 返回 true 表示系统处于安全状态

**5. `deadlock_request_safe(tid: usize, resource_id: usize) -> bool`**
- 提前检查：假设线程 tid 请求资源 resource_id 是否安全
- 在实际获取前验证系统状态
- 返回 true 表示可安全获取

**6. `deadlock_on_acquire(tid: usize, resource_id: usize)`**
- 线程成功获取资源后调用
- 更新 allocation 和 need 矩阵

**7. `deadlock_on_release(tid: usize, resource_id: usize)`**
- 线程释放资源时调用
- 更新 available 和 allocation 矩阵

#### 资源编号规则
- Mutex 资源 ID: 0 到 `mutex_count-1`
- Semaphore 资源 ID: `mutex_count` 到 `mutex_count + semaphore_count - 1`

---

### 2. `os/src/syscall/sync.rs` - 系统调用集成

#### 修改的系统调用

**1. `sys_enable_deadlock_detect(enabled: i32) -> i32`**
- 启用/禁用死锁检测
- 启用时调用 `deadlock_init_state()` 初始化

**2. `sys_mutex_create() -> u32`**
- 创建互斥锁后调用 `deadlock_on_create_mutex()`
- 更新资源索引映射和矩阵形状

**3. `sys_mutex_lock(mutex_id: usize) -> i32`**
- **关键改动**：在获取锁前检查死锁安全性
  ```rust
  if process.deadlock_enabled() && 
     !process.deadlock_request_safe(tid, resource_id) {
      return DEADLOCK_ERR; // -0xDEAD
  }
  ```
- 获取成功后调用 `deadlock_on_acquire()`

**4. `sys_mutex_unlock(mutex_id: usize) -> i32`**
- 解锁后调用 `deadlock_on_release()`
- 更新资源可用性

**5. `sys_semaphore_create(sem_count: u32) -> u32`**
- 创建信号量后调用 `deadlock_on_create_semaphore()`
- 更新资源索引和矩阵

**6. `sys_semaphore_down(sem_id: usize) -> i32`**
- **关键改动**：类似 mutex_lock，检查死锁安全性
  ```rust
  let resource_id = MUTEX_COUNT + sem_id;
  if process.deadlock_enabled() && 
     !process.deadlock_request_safe(tid, resource_id) {
      return DEADLOCK_ERR;
  }
  ```
- 操作成功后调用 `deadlock_on_acquire()`

**7. `sys_semaphore_up(sem_id: usize) -> i32`**
- 信号量增加后调用 `deadlock_on_release()`

#### 辅助函数
- `deadlock_on_create_mutex()`: 注册新 mutex，扩展矩阵
- `deadlock_on_create_semaphore()`: 注册新 semaphore，扩展矩阵

---

### 3. `os/src/syscall/thread.rs` - 线程注册

#### 修改的系统调用

**`sys_thread_create(entry: usize, arg: usize) -> u32`**
- 创建新线程后调用 `process.deadlock_register_thread(new_tid)`
- 确保矩阵行数与现有线程数一致
- 新线程的 allocation 和 need 行初始化为 0

---

## 工作原理

### 死锁检测流程

1. **启用死锁检测**
   ```
   sys_enable_deadlock_detect(1) → 初始化 available/allocation/need 矩阵
   ```

2. **尝试获取资源（mutex_lock 或 semaphore_down）**
   ```
   已启用？
   ├─ 是 → deadlock_request_safe() 评估是否安全
   │       ├─ 安全 → 继续获取，调用 deadlock_on_acquire()
   │       └─ 不安全 → 返回 -0xDEAD，不执行获取
   └─ 否 → 直接尝试获取（传统行为）
   ```

3. **释放资源（mutex_unlock 或 semaphore_up）**
   ```
   调用 deadlock_on_release() 更新矩阵，增加可用资源
   ```

### Banker 算法（安全性检查）

安全状态定义：存在一个线程执行序列，使得每个线程都能获得所需资源至完成。

检查步骤：
1. 复制 `available` 向量为 `work`
2. 初始化 `finish` 数组为全 false
3. 循环直到无进展：
   - 找一个线程 i，其 `finish[i] = false` 且 `need[i] <= work`
   - 若找到：`work += allocation[i]`，`finish[i] = true`
4. 若所有线程都能完成（`finish` 全 true）则系统安全

---

## 数据结构示例

假设系统有 2 个 mutex、1 个 semaphore、3 个线程：

```
资源编号:
  - Mutex 0,1: resource_id = 0, 1
  - Semaphore 0: resource_id = 2

available = [1, 1, 1]  // 每种资源各一个副本

allocation = [
  [1, 0, 0],  // 线程0 持有 mutex0
  [0, 1, 0],  // 线程1 持有 mutex1
  [0, 0, 1]   // 线程2 持有 sem0
]

need = [
  [0, 1, 0],  // 线程0 还需要 mutex1
  [1, 0, 0],  // 线程1 还需要 mutex0
  [0, 0, 0]   // 线程2 不需要其他资源
]
```

在此状态下，系统处于危险状态（可能死锁）。

---

## 测试场景

### `ch8_deadlock_mutex1` - 自互斥死锁
- 线程锁定同一 mutex 两次
- 期望：第二次 lock 返回 `-0xDEAD`

### `ch8_deadlock_sem1` - 信号量死锁
- 信号量初始值为 1，线程 down 两次
- 期望：第二次 down 返回 `-0xDEAD`

### `ch8_deadlock_sem2` - 循环等待
- 多线程形成资源循环依赖
- 期望：某个 lock/down 操作返回 `-0xDEAD`

---

## 返回值说明

| 操作 | 成功 | 失败（检测到死锁） |
|------|------|-------------------|
| `sys_mutex_lock` | 0 | -0xDEAD (-3725) |
| `sys_semaphore_down` | 0 | -0xDEAD (-3725) |
| `sys_enable_deadlock_detect` | 0 | 0 |

---

## 注意事项

1. **矩阵扩展**：线程和资源数量动态变化，需要动态扩展矩阵
2. **线程退出清理**（可选）：当前实现未在线程退出时清理其行，可在未来优化
3. **条件变量集成**（可选）：当前未处理 condvar_wait 过程中的 mutex 所有权转移
4. **资源计数**：互斥锁和信号量初始可用量均为 1（互斥特性）

---

## 相关常量

在 `os/src/syscall/sync.rs` 中定义：
```rust
const DEADLOCK_ERR: i32 = -0xDEAD; // -3725
```

在 `process.rs` 中：
```rust
const MUTEX_COUNT: usize = 16;         // 最大 mutex 数
const SEMAPHORE_COUNT: usize = 16;     // 最大 semaphore 数
const TASK_NUM: usize = 16;            // 最大线程数
```

