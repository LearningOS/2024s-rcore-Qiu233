## 实现功能
定义`TaskInfoBlock`类型保存任务第一次被调度的时刻和syscall总次数，在`TCB`中增加一个对应字段。具体实现如下：
* 使用`BTreeMap<usize, u32>`保存syscall次数，利用了原有的heap allocator
* 在syscall分派之前更新，所以`sys_task_info`得到的也包含本次syscall
* 在`__switch`之前(共有两处)检查下一个任务是否第一次被调度，如果是则将当前时刻记录进`TIB`


## 问答

### 一
使用的SBI是随仓库clone下来的`bootloader/rustsbi-qemu.bin`文件。

首先三者的共性是：都是exeption，触发trap，跳转至`stvec`中高62位设定的地址，即符号`__alltraps`处，执行一系列保存寄存器状态的代码后通过指令`call`调用`trap_handler`函数。该函数内，这三个测例都会调用`exit_current_and_run_next`函数结束当前任务并加载下一个任务执行。

三者具体情况：

* 测例`ch2b_bad_address`尝试往地址`0x0`处写入数据`0`，触发7号异常`Store/AMO access fault`。
* 测例`ch2b_bad_instructions`尝试执行S模式指令`sret`，触发2号异常`Illegal instruction`。
* 测例`ch2b_bad_register`尝试读取S模式寄存器`sstatus`，触发2号异常`Illegal instruction`。

### 二

#### 1
进入`__restore`时，有三种情况：
1. 从`run_first_task`开始执行，第一个任务首次调度，从`__switch`的最后一条指令`ret`进入。此时`a0`的值是boot stack上的地址(即`_unused: TaskContext`的地址)。
2. 非第一个任务，但同样是首次调度，由`run_next_task`触发，同样从`__switch`的最后一条指令`ret`进入。此时`a0`的值是`TaskManager`中前一个任务的`TaskContext`的地址。
3. 从`__alltraps`进入，在`call trap_handler`返回之后继续向下执行进入`__restore`，此时`a0`是`trap_handler`返回的指针，与前一条指令`mv a0, sp`传入的地址没有区别，因为`trap_handler`直接返回了原引用。

`__restore`的两种使用场景：
1. 从S模式切换至U模式(`sret`指令)，对应到前面两种情况，区别只是进入时a0指向的地址区域不同。
2. 在U模式下触发trap进入S模式，保存状态并执行`trap_handler`之后恢复状态，对应到第三种情况。

与第二章的不同在于第三章的`__restore`开头没有`mv sp, a0`指令，因为`sp`已经在`__switch`后半段设置为内核栈上的`TrapContext`的指针，只需要直接跳转至`__restore`即可，无需通过`a0`传递。

#### 2
* `sstatus`：其中的SPP bit进入U模式之前必须为0，分两种情况：1.U模式下发生trap，此时43和46行恢复前面保存的值，而U模式进入trap前CPU已自动将SPP设置为0，所以为0；2.从`__switch`进入，即首次调度，`app_init_context`函数已将SPP对应的位置设置为0。
* `sepc`：U模式发生异常时保存的，从trap handler返回到的地址。如果是首次调度，那么`sepc`保存的是**任务的入口点**。
* `sscratch`：对一个任务来说，在U模式下保存kernel sp，在S模式下保存user sp。具体用途是通过指令`csrrw sp, sscratch, sp`交换kernel sp和user sp，分别在`__alltraps`的头部和`__restore`的尾部。

#### 3
根据资料[registers](https://en.wikichip.org/wiki/risc-v/registers)首先`x2`就是`sp`，会在`sret`执行之前被恢复，也就是如下两条代码：

```
addi sp, sp, 34*8
csrrw sp, sscratch, sp
```

根据搜到的[讨论](https://groups.google.com/a/groups.riscv.org/g/sw-dev/c/cov47bNy5gY)，`x4`是thread pointer，用途是指向线程独有的数据，而我们目前的操作系统完全是单核的，所以不需要保存。

#### 4
该指令交换`sscratch`和`sp`的值，交换后：
* `sp`是user stack的栈顶
* `sscratch`是kernel stack的栈顶

#### 5
`__restore`的最后一条指令`sret`将会从S模式切换至U模式，当`sret`执行时若`sscratch.SPP=0`则进入用户态，代码中有两种情况，均满足条件：
* 从`__alltraps`进入，即从U模式进入S模式前CPU已经把SPP设置为0，然后被代码保存，在`__restore`中恢复，所以还是0
* 从`__switch`进入，即任务的首次调度，在`app_init_context`中SSP被设置为0

参考资料：risc-v-privileged-v1.10第4.1.1小节

#### 6
该指令交换`sscratch`和`sp`的值，交换后：
* `sscratch`是user stack的栈顶
* `sp`是kernel stack的栈顶

#### 7
从U模式进入S模式的过程叫做trap，有两种情况：

1. 同步的异常(exception)，由执行的指令触发，例如U模式下ecall指令会触发编号为8的异常，ebreak会触发编号为3的异常。这种情况下发生trap的指令与发生的trap类型是直接相关的。
2. 异步的中断(interrupt)，一般不由执行的某条指令触发，例如时钟中断。发生时由CPU选择一条指令发生trap。这种情况不能确定trap在哪条指令触发。

实际上`__alltraps`的第一条指令`csrrw sp, sscratch, sp`会写入S模式寄存器`sscratch`，所以此时必定已经进入S模式，否则该指令会失败。

参考资料：
* riscv-spec-v2.2第1.3节
* [riscv-priv-isa-manual/Priv-v1.12/supervisor](https://five-embeddev.com/riscv-priv-isa-manual/Priv-v1.12/supervisor.html#sec:scause)第4.1.8小节、4.1.6小节


# 荣誉准则
1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 以下各位 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：
* 无。直到完成为止，我未与任何人交流本章内容。

2. 此外，我也参考了以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

* 有关RISC-V的**非代码**参考资料已经在本篇文档中给出，主要来自RISC-V官方手册。
* 代码实现并未参考任何已有资料，未使用任何AI生成的代码，代码实现全部由我个人完成。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。
