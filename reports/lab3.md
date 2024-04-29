# 实现功能

* 实现`sys_spawn`，相当简单，只需要用ELF文件的字节流，仿照原有生成进程的代码即可。
* `stride`调度，利用原理`a.wrapping_add(b) < a`判断是否溢出，溢出时全部减去最小的stride，这个策略在BIG_STRIDE不是特别大时很有效。
* 选做：实现fork的Copy-On-Write，利用`Arc<FrameTracker>`让不同任务可以引用同一个Frame，在StorePageFault发生时检查权限并复制。用`Arc::try_unwrap`检查并获得最后一个引用的所有权，此时不复制。
* 选做：多核调度，具体如下：

# 多核调度

为了保证基础的同步，有几个问题需要解决：
* 用第三方库`spin`的`Mutex`代替原有的`UPSafeCell`，该库的Mutex默认情况下是公平的，即具体实现是类型`TicketMutex`。
* 原有代码逻辑导致的死锁，我已在解决的代码中留下大量注释说明这个问题，请见`exit_current_and_run_next`和`sys_waitpid`。简单来说任务加锁之后不能直接锁定子任务，否则多个hart异步执行时几乎必定导致死锁。
* 调度的同步，请见本节最后。

核心(hart)的寻找：
> 在`entry.asm`中保存寄存器a1传入的dtb指针，保存寄存器a0传入的boot hartid。  
> 用第三方库`hermit-dtb`解析`cpus`节点，找到所有形如`cpu@n`的节点，`n`就是可用的hartid。  

初始化：
> 利用SBI函数`sbi_hart_start`将除了boot hart以外的所有hart全部启动，经过`hart.asm`中`_hart_start`跳转至`hart.rs`中`hart_main`。  
> `hart_main`会用传入的`opaque`参数和boot hart同步，保证hart初始化完毕。原理是利用`AtomicUsize`。

HartLocalData：
> 为每个hart在kernel heap上分配一个独有、生命周期为整个系统的`HartLocalData`，包括hartid和`Processor`实例。  
> 原本的全局Processor实例被移除，boot hart也有自己的HLC。  
> `HartLocalData`的指针保存在`tp`寄存器中，切换到用户态时保存在`TrapContext`新增的字段上。`__restore`中加入保存`tp`的代码，`__alltraps`中加入恢复内核`tp`和保存用户`tp`的代码。  
> HLD的获取封装成函数`get_hartid`和`get_processor`，内部实现是读取tp寄存器，hart调度时能够直接用`get_processor`得到自己的Processor实例。

调度的同步：
> `TaskContext`中加入字段`lock: AtomicUsize`，目的是为了解决这个问题：  
> 一个hart会在保存`TaskContext`之前把TCB返回全局的Manager，导致可能在其他hart准备调度该任务时`TaskContext`还没有被前一个hart保存完毕。  
>
> 解法如下：  
> `__switch`保存前一个`TaskContext`后，会利用`LR/SC`将`lock`切换为0，表示释放。  
> 在恢复下一个`TaskContext`前利用`LR/SC`将下一个`TaskContext`的lock从0切换为1，这里必须检查并等待lock为0，表示上一个调度它的hart已经释放。  
> 注：为了使用原子指令，我在`switch.S`的头部加了对`A`扩展的声明，不过因为项目原本的编译目标是`gc`，所以并没有兼容性问题。

# 问答作业
stride算法深入：

利用反证法，如果两个任务`a`和`b`的stride之差大于`BIG_STRIDE / 2`，不妨设`a`比`b`小，因为prio不能小于2，所以pass不能大于`BIG_STRIDE / 2`，那么在`b`上一次被调度时`a`必定已经小于`b`了，可是依假设可知被调度的仍然是`b`，这违反了stride算法的原则，即更小的先被调度，证毕。

代码表示如下

``` Rust
impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.stride < other.stride && other.stride - self.stride > BIG_STRIDE / 2 {
            Some(Ordering::Less)
        } else if other.stride < self.stride && self.stride - other.stride > BIG_STRIDE / 2 {
            Some(Ordering::Greater)
        } else {
            None
        }
    }
}
```


# 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与以下各位就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

* 无。直到完成为止，我未与任何人交流本章内容。

2. 此外，我也参考了以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

* dtb参考手册`devicetree-specification-v0.4`，未找到在线版本，我使用的是下载的pdf版本。
* 原子指令参考手册[Priv](https://five-embeddev.com/riscv-user-isa-manual/Priv-v1.12/a.html)和文章[RISC原子指令介绍-泰晓科技](https://tinylab.org/riscv-atomics/)。
* 代码实现并未参考任何已有资料，未使用任何AI生成的代码，代码实现全部由我个人完成。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。
