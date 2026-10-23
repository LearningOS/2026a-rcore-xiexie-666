# lab1 实验报告（chapter3：获取任务信息）

## 一、功能实现总结

在 `TaskControlBlock` 中新增一张按系统调用编号下标的调用次数计数表，并在 `trap_handler`
于系统调用分发**之前**为当前任务自增对应计数，使得 `sys_trace` 查询自身这次调用时也能被计入。
`sys_trace` 按 `trace_request` 分三路：0 把 `id` 当 `*const u8` 读出该地址一字节返回；
1 把 `data` 的低字节写入 `id` 地址并返回 0；2 返回当前任务调用编号为 `id` 的系统调用次数；
其余请求返回 -1。

## 二、简答作业

### 2.1 U 态程序的越权访问行为

实验环境：QEMU 8.2.2，SBI 为 **RustSBI 0.4.1**（`bootloader/rustsbi-qemu.bin`，
启动日志中打印 `RustSBI version 0.4.1`）。

三个 bad 测例的实际运行结果如下（BASE=2 编译，与基础测例一同加载执行）：

| 测例 | 做了什么 | 实际输出 |
| --- | --- | --- |
| `ch2b_bad_address.rs` | `(0x0 as *mut u8).write_volatile(0)`，向非法地址 0 写入 | `[kernel] PageFault in application, bad addr = 0x0, bad instruction = 0x804003a4, kernel killed it.` |
| `ch2b_bad_instructions.rs` | 在内核态才允许的指令位置执行 `sret` | `[kernel] IllegalInstruction in application, kernel killed it.` |
| `ch2b_bad_register.rs` | `csrr {}, sstatus`，直接读 S 态寄存器 sstatus | `[kernel] IllegalInstruction in application, kernel killed it.` |

可以看到程序确实已经运行在 U 态：普通用户程序能正常打印和计算，而一旦访问 S 态专用的特权指令或
CSR，就会触发异常。具体来说，向未映射/无权限的地址写入触发了 store page fault，
其 `stval` 为出错地址 `0x0`、`sepc` 为出错指令地址 `0x804003a4`；
而 `sret` 和 `csrr ..., sstatus` 在 U 态都是非法指令（U 态无权执行 S 态特权指令、无权访问 S 态 CSR），
所以都表现为 illegal instruction 异常。

这三种情况最终都被内核捕获并直接杀死该用户任务（`exit_current_and_run_next`），
而不是让整个系统崩溃，说明 trap 机制已经把用户态的越权行为拦在了 U/S 边界上。
`ch2b_bad_address` 中那条注释也说明这一点与 SBI 版本有关：老版本 SBI 在该情形下无法正常退出，
RustSBI 0.4.1 下内核能够正常处理并继续调度下一个任务。

### 2.2 深入理解 `trap.S` 中的 `__alltraps` 与 `__restore`

#### (1) L40：刚进入 `__restore` 时 `sp` 代表什么值？`__restore` 的两种使用情景？

刚进入 `__restore` 时，`sp` 指向**该任务内核栈上刚分配好的 `TrapContext` 的起始地址**
（即 `addi sp, sp, -34*8` 之后的那块 34×8 字节区域的低地址端），此时 `sscratch` 中暂存着用户栈栈顶。
这块 `TrapContext` 里保存着该任务被陷入时的 32 个通用寄存器以及 `sstatus`、`sepc`。

`__restore` 有两种使用情景：

1. **从 trap 返回**：`__alltraps` 保存完现场后 `call trap_handler`，`trap_handler` 处理完异常
   （可能是系统调用、时钟中断或杀死任务）返回后，控制流落到 `__restore`，把内核栈上的
   `TrapContext` 恢复回寄存器并返回用户态继续执行。
2. **第一次进入用户态**：`init_app_cx` 在用户栈顶伪造好 `argc/argv`，把 `sepc` 设成应用入口、
   把 `sstatus` 的 SPP 清 0（表示 sret 后进入 U 态），然后 `__switch` 直接跳到 `__restore`。
   此时并没有真正发生过 trap，`TrapContext` 是人为构造的，但 `__restore` 不区分这两种来源，
   照样"返回用户态"，于是任务第一次跑起来。

#### (2) L43-L48：特殊处理了哪些寄存器？它们对进入用户态的意义？

这几行特殊处理了 **`sstatus`、`sepc`、`sscratch`** 三个寄存器（通过 `t0/t1/t2` 中转）：

```asm
ld t0, 32*8(sp)     # sstatus
ld t1, 33*8(sp)     # sepc
ld t2, 2*8(sp)      # 用户 sp
csrw sstatus, t0
csrw sepc, t1
csrw sscratch, t2
```

- **`sstatus`**：S 态状态寄存器。其中 `SPP` 位决定 `sret` 之后回到哪个特权级（0=U 态，1=S 态），
  `SPIE` 决定 `sret` 之后是否开中断，`SUM` 决定 S 态能否访问 U 态内存。要进入用户态，
  必须让它恢复到 SPP=0 的取值，否则 `sret` 会跑回 S 态。
- **`sepc`**：发生 trap 时的 PC。`sret` 会把 `sepc` 装进 `pc`，所以它决定用户态从哪条指令继续执行；
  第一次运行任务时正是靠它把入口地址送进 PC。
- **`sscratch`**：保存用户栈指针。`__alltraps` 一进来就把 `sp` 与 `sscratch` 交换，
  用户栈被暂存到了 `sscratch` 里。返回前必须把它写回 `sscratch`，
  这样下一次从 U 态陷入时 `csrrw sp, sscratch, sp` 才能重新换出内核栈、并把用户 `sp` 暂存好。

注意 `sp`(x2) 没有在这里从 `TrapContext` 直接恢复，而是先经由 `t2` 存进 `sscratch`，
留到最后一条 `csrrw` 统一交换。

#### (3) L50-L56：为何跳过了 x2 和 x4？

```asm
ld x1, 1*8(sp)
ld x3, 3*8(sp)
.set n, 5
.rept 27
    LOAD_GP %n
    .set n, n+1
.endr
```

- **`x2` 是 `sp`**。此时 `sp` 正指向内核栈上的 `TrapContext`，后面还要靠它读取剩余寄存器、
  并在 `addi sp, sp, 34*8` 中释放 `TrapContext`。如果在这里 `ld x2` 把用户栈指针装进 `sp`，
  就再也找不回 `TrapContext` 和内核栈了。所以用户 `sp` 走的是"存进 `sscratch`、
  最后由 `csrrw sp, sscratch, sp` 一次性换进 `sp`"的路径，不能在这里恢复。
- **`x4` 是 `tp`（线程指针）**。本实验的用户程序不使用 `tp`，U 态与 S 态下 `tp` 的语义也不一样，
  内核既没有在 `__alltraps` 里保存它，自然也不需要在这里恢复它。

（`x0` 恒为零寄存器，不参与保存/恢复，因此循环从 `x5` 开始、覆盖 `x5~x31`。）

#### (4) L60 与 L13：`csrrw sp, sscratch, sp` 之后 `sp` 与 `sscratch` 的含义

**L60（`__restore` 中最后一条 `csrrw`）**：执行前 `sp` = 内核栈上 `TrapContext` 的顶部
（已经过 `addi sp, sp, 34*8` 释放），`sscratch` = 用户栈顶。执行后两者交换：
`sp` = 用户栈顶（回到用户栈），`sscratch` = 内核栈顶。
这个状态正好是"下一次从 U 态陷入 S 态"所期望的初始状态，于是紧接着的 `sret` 就能安全地回到用户态。

**L13（`__alltraps` 中的第一条 `csrrw`）**：执行前 `sp` = 用户栈顶，`sscratch` = 内核栈顶
（正是上一次返回用户态时留下的状态）。执行后 `sp` = 内核栈顶，
于是后面 `addi sp, sp, -34*8` 能在内核栈上分配 `TrapContext`；
`sscratch` = 用户栈顶，作为暂存，稍后由 `csrr t2, sscratch` 读出并写入 `TrapContext` 的 `2*8` 位置。

可以看到，这两处 `csrrw sp, sscratch, sp` 互为逆操作，`sscratch` 在其中扮演"用户栈指针的交换区"这一角色。

#### (5) `__restore` 中状态切换在哪一条指令？为何执行之后会进入用户态？

在 **`sret`**（`__restore` 的最后一条指令）。它之前已经把 `sstatus`、`sepc`、`sscratch`
和全部通用寄存器恢复到陷入前的值，其中 `sstatus.SPP = 0`、`sepc` 指向用户态下一条指令。
`sret` 的硬件语义是：把 `pc` 置为 `sepc`，按 `sstatus.SPP` 决定返回到 U 态还是 S 态
（SPP=0 则进入 U 态），同时把 `SIE` 置为 `SPIE`、把 `SPP` 清 0。
因为此时 SPP=0、sepc 指向用户代码，所以 `sret` 执行完之后 CPU 就运行在 U 态、
从用户程序的下一条指令继续执行，状态切换就此完成。

#### (6) 从 U 态进入 S 态是哪一条指令？

是用户程序里的 **`ecall`** 指令。用户态执行 `ecall` 时，硬件会产生
"environment call from U-mode" 异常，把当前 `pc` 存入 `sepc`、
把 `sstatus.SPP` 置 1、跳到 `stvec` 指向的 `__alltraps`，
于是 CPU 进入 S 态并从 `__alltraps` 开始执行。
（严格来说，特权级的切换是硬件在异常触发那一刻完成的，`ecall` 是引发这次切换的指令。）

## 三、荣誉准则

我承诺：本次实验由我本人独立完成，实验代码由我本人编写和调试，
实验报告中的分析与结论均基于我自己在实验环境中的实际观察与思考；
我没有抄袭他人代码，也没有把自己的代码提供给他人，更没有伪造实验结果。
我理解并接受，若违反上述承诺，本次实验成绩按 0 分计。

## 四、对本次实验的看法（可选）

本章工作量适中，核心难点不在代码量，而在于想清楚两件事：
一是"统计点放在哪里"——只有在 `trap_handler` 分发之前自增，`sys_trace` 才能看到自己这一次调用，
测例里 `count_syscall(SYSCALL_TRACE)` 第一次返回 2、第二次返回 7 才会成立；
二是 TCB 结构变大后要留意它是在内核启动栈上构造的，`MAX_APP_NUM × 计数表` 的尺寸不能随意放大。
简答题部分要求逐行读懂 `trap.S`，尤其是 `sscratch` 与 `sp` 的交换、以及 `__restore` 的两种调用来源，
这两点在没有 GDB 单步的情况下确实需要花些时间在纸上推演。
建议可以把 `TrapContext` 的字段偏移做成一张表格附在文档里，会省掉很多翻 `trap.S` 的时间。
