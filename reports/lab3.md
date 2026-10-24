# 实验三报告：进程管理与调度（ch5）

## 一、功能实现总结（200 字以内）

本章做三件事。一是把上一章的系统调用迁移到新的进程结构下：`sys_get_time` 不再解引用用户指针，改为按当前进程 token 查页表逐字节写入，因此 `TimeVal` 跨页也能正确写入；`sys_mmap`/`sys_munmap` 在 `MemorySet` 上新增"按页对齐插入（遍历已有区间做重叠检测、跳过 sbrk 留下的空占位区、把 `prot` 的 bit0/1/2 映射成 R/W/X 并补 U 位）"和"精确匹配摘除（清页表项并归还物理页）"两个接口。二是实现 `sys_spawn`(400)：按名字取 ELF 直接新建地址空间和 TCB，不复制父进程地址空间，挂到父进程 `children` 后加入就绪队列。三是 stride 调度：TCB 增加 `priority`（初始 16）与 `stride`（初始 0），`pass = BIG_STRIDE / priority`；就绪队列由 FIFO 改为以 `(stride, pid)` 为键的 `BTreeMap`，每次取最小者并在取出时立刻累加 pass；`sys_set_priority`(140) 拒绝 `prio < 2`，成功返回新的 prio。

## 二、问答作业

### 1. p2 执行一个时间片后，实际会轮到 p1 吗？

不会，实际仍然是 p2 被调度。

`stride` 用 8 位无符号数存储时，p2 的 stride 从 250 加上 pass=10 得 260，而 260 超出 8 位表示范围，模 256 后变成 4；p1 的 stride 仍然是 255。如果比较器直接按无符号整数大小比较，就有 4 < 255，于是调度器认为 p2 的 stride 最小，又选中 p2。p2 连续运行，p1 被饿死。

这就是 stride 算法的"反转"现象：溢出把本来的最大值变成了最小值，破坏了"stride 越小越应先运行"这一单调性。所以 8 位存储 + BigStride=255 时，必须给 `Stride` 写一个能识别环绕的比较器，而不能直接用无符号大小比较。

### 2. 为什么在不考虑溢出时，`priority >= 2` 能保证 `STRIDE_MAX - STRIDE_MIN <= BigStride / 2`？

用归纳法说明，记 `B = BigStride`。

基础情况：所有进程初始 stride 都是 0，极差为 0，结论成立。

归纳步：假设某时刻所有就绪进程的 stride 都落在区间 `[m, m + B/2]` 内（`m` 是当前最小 stride，极差 ≤ B/2）。调度器选中的正是 stride 为 `m` 的进程，给它加上 `pass = B / priority`。因为 `priority >= 2`，所以 `pass <= B/2`，该进程的新 stride 至多为 `m + B/2`，仍然不超过原来的上界。而其它进程的 stride 没有变化，被选中的进程 stride 只增不减，所以新的最小值依然是 `m`。于是新的极差不超过 `B/2`。

换句话说，`priority >= 2` 这个约束的作用就是把单次增量限制在"半圈"以内，保证最大值和最小值之间的差距永远不超过半个 `B`。正因为极差不超过 `B/2`，任意两个 stride 的真实差值都落在 `(-B/2, B/2]` 中，比较时只要发现差值超出了这个区间，就说明计算过程中发生过环绕，把比较结论反过来即可。

### 3. 溢出情况下 `partial_cmp` 的补全

```rust
use core::cmp::Ordering;

/// 8 bit 存储 stride 时取 255，与题面示例一致
const BIG_STRIDE: u64 = 255;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let half = (BIG_STRIDE / 2) as i64;
        // 直接相减：如果没有环绕，差的符号就是真实的先后关系
        let diff = self.0 as i64 - other.0 as i64;
        Some(if diff > half {
            // self 比 other 大出半圈以上，说明 other 已经绕过一轮，真实关系是 self < other
            Ordering::Less
        } else if diff < -half {
            // 反过来：self 已经绕过一轮，真实关系是 self > other
            Ordering::Greater
        } else {
            diff.cmp(&0)
        })
    }
}

impl PartialEq for Stride {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
```

例如 p1.stride=255、p2 溢出后为 4：`diff = 4 - 255 = -251`，`-251 < -127`，判定 p2 `Greater` than p1，于是下一次仍然先调度 p1，与理论一致。

## 三、荣誉准则

我承诺：本次实验由我本人独立完成，实验代码由我本人编写和调试，
实验报告中的分析与结论均基于我自己在实验环境中的实际观察与思考；
我没有抄袭他人代码，也没有把自己的代码提供给他人，更没有伪造实验结果。
我理解并接受，若违反上述承诺，本次实验成绩按 0 分计。

## 四、对本次实验的看法（可选）

- 本章的代码量不算大，但"进程"这个概念一引入，改动面就很广：`fork`/`exec`/`spawn`/`waitpid` 之间的关系、僵尸进程的回收、子进程在父进程退出后挂到 initproc 下，都要一起对上，所以真正花时间的是读懂新的 TCB 结构，而不是写那几十行调度代码。
- stride 部分最容易出错的地方不是算法本身，而是"stride 什么时候累加"。我把它放在调度器取出进程的位置（`fetch` 里累加 pass），而不是放在进程被换出的时候，这样进程只要被调度过一次就一定记了账，不会因为异常退出或阻塞而漏记。
- 就绪队列用 `BTreeMap<(stride, pid), TCB>` 而不是 `BTreeMap<stride, TCB>`，是因为很多进程初始 stride 都是 0，用 stride 单键会互相覆盖；用 pid 做第二关键字既保证了键唯一，也让 stride 相同时的行为是确定的（小 pid 先跑）。
- 优先级改变时我没有重置 stride。理论上进程刚创建时的累积量很少，实测 6 个进程的 `count/prio` 比值差异也远小于 1.5，所以保持了"stride 是累计运行长度"的原始语义。
