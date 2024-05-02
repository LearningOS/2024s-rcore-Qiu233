# 实现功能

## 必做题
在`Inode`里面将direct数量减一，改成字段保存link数量，保持`Inode`长度不变。

三个系统调用：
* `linkat`：即增加新的entry，指向已有的inode，link数加一。
* `unlinkat`：即删除entry，如果删除时inode的link数只有1，那么会删除inode。
* `fstat`：只需要计算inode id，加上之前保存link数即可。

## 选做题
重写内存管理，实现Demand Paging，由于增加了`mmap`的参数，所以应该过不了CI，但本地改动lib.rs中mmap定义后，实测`ch6_usertest`可以过，所以实现并无问题，以下所有功能都已通过测试：
1. 扩展`mmap`支持文件映射：
    * 以shared模式映射文件(的页)，在进程结束(映射被释放)时、或munmap发生时会被写回文件(`msync`暂未实现)，进程之间可以用这种方式共享内存。
    * shared映射不能使文件变大，内存区域超出文件大小的部分初始时全为0，这部分不会被写回文件。
    * 以private模式映射文件(的页)，不会被写回文件，在内存发生写入时才会(COW)复制到新的Frame。
    * 与block cache同步：`OSInode`被写入时会使`MFile`的映射页面无效化，使其从block cache中同步改动。由于`MFile`基于block cache实现，所以block cache无需反过来从`MFile`同步改动。
    * shared和private模式都是lazy load，即页面真正被使用时才会分配Frame并加载，当文件页所有映射都被释放时，Frame也会被释放。
2. 支持Demand Paging：ELF文件的LOAD部分以**private模式**映射，因此本身具有lazy load性质。此外这种策略允许同一个ELF文件的多个进程始终共享同一份只读区域，节省内存。
3. Lazy内存分配：现在除了`TRAP_CONTEXT_BASE`和内核栈以外的所有Framed区域初始时都没有分配Frame，直到发生缺页中断时才会分配。
4. fork的COW：
    * fork得到的分支进程之间会共享页面，直到发生写入为止。
    * 由于现在ELF文件以private模式映射，所以真正COW的页面只有用户栈、brk、mmap申请的非文件映射区域。
    * 文件映射区域可以直接"复制"，因为内核保证每个不同的文件页最多只存在一个对应的Frame，不会浪费内存。

# 问答作业
1. root inode是根目录，类比linux的`/`。根目录损坏意味着损失所有下级目录及文件。

ch7：
1. 著名软件CheatEngine的主程序是用Pascal写的，但是它有很多周边程序，例如`DotNetDataCollector`是C++写的，两者之间的通信就用Pipe完成，请见[源码](https://github.com/cheat-engine/cheat-engine/blob/master/Cheat%20Engine/DotNetDataCollector/DotNetDataCollector/PipeServer.cpp)。
2. 例如`wc -l < Cargo.toml`命令会输出文件`Cargo.toml`的行数。
3. 共享内存(Shared Memory)，例如linux上用`mmap`和`MAP_SHARED`参数打开同一个文件即可在同一块内存中通信，不过会需要额外的同步机制。

# 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与以下各位就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

* 无。直到完成为止，我未与任何人交流本章内容。

2. 此外，我也参考了以下资料 ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

* 问答题参考了`wc --help`的输出。CheatEngine源码，不过主要是2017年时读过，碰巧问答题问起Pipe的实际用途就想起来了。
* 代码实现并未参考任何已有资料，未使用任何AI生成的代码，代码实现全部由我个人完成。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。
