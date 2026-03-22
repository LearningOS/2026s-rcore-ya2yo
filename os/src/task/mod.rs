//! Task management implementation
//!
//! Everything about task management, like starting and switching tasks is
//! implemented here.
//!
//! A single global instance of [`TaskManager`] called `TASK_MANAGER` controls
//! all the tasks in the operating system.
//!
//! Be careful when you see `__switch` ASM function in `switch.S`. Control flow around this function
//! might not be what you expect.

mod context;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use crate::loader::{get_app_data, get_num_app};
use crate::sync::UPSafeCell;
use crate::trap::TrapContext;
use alloc::vec::Vec;
use lazy_static::*;
use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};

pub use context::TaskContext;

use crate::mm::{VirtAddr,PhysAddr,PageTable,MapPermission};
use crate::config::PAGE_SIZE;

/// The task manager, where all the tasks are managed.
///
/// Functions implemented on `TaskManager` deals with all task state transitions
/// and task context switching. For convenience, you can find wrappers around it
/// in the module level.
///
/// Most of `TaskManager` are hidden behind the field `inner`, to defer
/// borrowing checks to runtime. You can see examples on how to use `inner` in
/// existing functions on `TaskManager`.
pub struct TaskManager {
    /// total number of tasks
    num_app: usize,
    /// use inner value to get mutable access
    inner: UPSafeCell<TaskManagerInner>,
}

/// The task manager inner in 'UPSafeCell'
struct TaskManagerInner {
    /// task list
    tasks: Vec<TaskControlBlock>,
    /// id of current `Running` task
    current_task: usize,
}

lazy_static! {
    /// a `TaskManager` global instance through lazy_static!
    pub static ref TASK_MANAGER: TaskManager = {
        println!("init TASK_MANAGER");
        let num_app = get_num_app();
        println!("num_app = {}", num_app);
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        for i in 0..num_app {
            tasks.push(TaskControlBlock::new(get_app_data(i), i));
        }
        TaskManager {
            num_app,
            inner: unsafe {
                UPSafeCell::new(TaskManagerInner {
                    tasks,
                    current_task: 0,
                })
            },
        }
    };
}

impl TaskManager {
    /// Run the first task in task list.
    ///
    /// Generally, the first task in task list is an idle task (we call it zero process later).
    /// But in ch4, we load apps statically, so the first task is a real app.
    fn run_first_task(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        let next_task = &mut inner.tasks[0];
        next_task.task_status = TaskStatus::Running;
        let next_task_cx_ptr = &next_task.task_cx as *const TaskContext;
        drop(inner);
        let mut _unused = TaskContext::zero_init();
        // before this, we should drop local variables that must be dropped manually
        unsafe {
            __switch(&mut _unused as *mut _, next_task_cx_ptr);
        }
        panic!("unreachable in run_first_task!");
    }

    /// Change the status of current `Running` task into `Ready`.
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let cur = inner.current_task;
        inner.tasks[cur].task_status = TaskStatus::Ready;
    }

    /// Change the status of current `Running` task into `Exited`.
    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let cur = inner.current_task;
        inner.tasks[cur].task_status = TaskStatus::Exited;
    }

    /// Find next task to run and return task id.
    ///
    /// In this case, we only return the first `Ready` task in task list.
    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..current + self.num_app + 1)
            .map(|id| id % self.num_app)
            .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
    }

    /// Get the current 'Running' task's token.
    fn get_current_token(&self) -> usize {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_user_token()
    }

    /// Get the current 'Running' task's trap contexts.
    fn get_current_trap_cx(&self) -> &'static mut TrapContext {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_trap_cx()
    }

    /// Change the current 'Running' task's program break
    pub fn change_current_program_brk(&self, size: i32) -> Option<usize> {
        let mut inner = self.inner.exclusive_access();
        let cur = inner.current_task;
        inner.tasks[cur].change_program_brk(size)
    }

    /// Switch current `Running` task to the task we have found,
    /// or there is no `Ready` task and we can exit with all applications completed
    fn run_next_task(&self) {
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task = next;
            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            drop(inner);
            // before this, we should drop local variables that must be dropped manually
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // go back to user mode
        } else {
            panic!("All applications completed!");
        }
    }

    /// Add syscall times
    fn syscall_counts(&self, id:usize){
        let mut inner=self.inner.exclusive_access();
        let current=inner.current_task;
        inner.tasks[current].syscall_counts[id]+=1;
    }
    /// get syscall times
    fn get_syscall_counts(&self, id:usize)->usize{
        let inner=self.inner.exclusive_access();
        let task=inner.current_task;
        inner.tasks[task].syscall_counts[id]
    }
    /// mmap
    fn mmap(&self,start: usize, len: usize, port: usize)->isize {
        if start % PAGE_SIZE != 0 {
        return -1;
        }

        // prot & !0x7 != 0 
        // prot & 0x7 == 0
        if (port & !0x7 != 0) || (port & 0x7 == 0) {
            return -1;
        }

        // prot: bit 0 (R), 1 (W), 2 (X)
        // MapPermission: R=1<<1, W=1<<2, X=1<<3, U=1<<4
        let mut permission = MapPermission::U;
        if (port & 1) != 0 { permission |= MapPermission::R; }
        if (port & 2) != 0 { permission |= MapPermission::W; }
        if (port & 4) != 0 { permission |= MapPermission::X; }

        let mut inner = TASK_MANAGER.inner.exclusive_access();
        let task=inner.current_task;
        
        // 长度向上对齐
        let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        
        inner.tasks[task].memory_set.mmap(start, len_aligned, permission)
    }
    /// munmap
    fn munmap(&self,start: usize, len: usize)->isize{
        if start % PAGE_SIZE !=0{
        return -1;
        }
        let len_aligned = ( len + PAGE_SIZE - 1) & !(PAGE_SIZE-1);

        let mut inner=TASK_MANAGER.inner.exclusive_access();
        let task=inner.current_task;
        inner.tasks[task].memory_set.munmap(start, len_aligned)
    }
}

/// Run the first task in task list.
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// Switch current `Running` task to the task we have found,
/// or there is no `Ready` task and we can exit with all applications completed
fn run_next_task() {
    TASK_MANAGER.run_next_task();
}

/// Change the status of current `Running` task into `Ready`.
fn mark_current_suspended() {
    TASK_MANAGER.mark_current_suspended();
}

/// Change the status of current `Running` task into `Exited`.
fn mark_current_exited() {
    TASK_MANAGER.mark_current_exited();
}

/// Suspend the current 'Running' task and run the next task in task list.
pub fn suspend_current_and_run_next() {
    mark_current_suspended();
    run_next_task();
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next() {
    mark_current_exited();
    run_next_task();
}

/// Get the current 'Running' task's token.
pub fn current_user_token() -> usize {
    TASK_MANAGER.get_current_token()
}

/// Get the current 'Running' task's trap contexts.
pub fn current_trap_cx() -> &'static mut TrapContext {
    TASK_MANAGER.get_current_trap_cx()
}

/// Change the current 'Running' task's program break
pub fn change_program_brk(size: i32) -> Option<usize> {
    TASK_MANAGER.change_current_program_brk(size)
}

/// Add syscall times
pub fn syscall_counts(id:usize){
    TASK_MANAGER.syscall_counts(id);
}

/// get syscall times
pub fn get_syscall_counts(id:usize)->usize {
    TASK_MANAGER.get_syscall_counts(id)
}

/// sys_trace
pub fn task_sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    let token = current_user_token();
    let page_table = PageTable::from_token(token);

    match trace_request {
        // 功能 0: 读内存 (User, Readable)
        0 => {
            let va = VirtAddr::from(id);
            let vpn = va.floor();
            
            if let Some(pte) = page_table.translate(vpn) {
                // 必须是有效的、用户态的、可读的
                if !pte.is_valid() || !pte.readable() || !pte.user(){
                    return -1;
                }
                
                let offset = va.page_offset();
                let pa:PhysAddr = pte.ppn().into();
                let target_pa = PhysAddr::from(pa.0 + offset);
                
             
                    // 读取一个字节
                    let val = *target_pa.get_mut::<u8>();
                    val as isize
                
            } else {
                -1
            }
        },
        
        // 功能 1: 写内存 (User, Writable)
        1 => {
            let va = VirtAddr::from(id);
            let vpn = va.floor();
            
            if let Some(pte) = page_table.translate(vpn) {
                // 必须是有效的、用户态的、可写的
                if !pte.is_valid() || !pte.writable() || !pte.user() {
                    return -1;
                }
                
                let offset = va.page_offset();
                let pa:PhysAddr = pte.ppn().into();
                let target_pa = PhysAddr::from(pa.0 + offset);
                

                    *target_pa.get_mut::<u8>() = data as u8;
                
                0
            } else {
                -1
            }
        },
        
        // 功能 2: 查询系统调用次数 (必须保留!)
        // 如果你的 TaskControlBlockInner 中没有 syscall_times 字段，需要在那里加上
        2 => {
            let inner = TASK_MANAGER.inner.exclusive_access();
            let task=inner.current_task;
            if id < inner.tasks[task].syscall_counts.len() {
                inner.tasks[task].syscall_counts[id] as isize
            } else {
                -1
            }
        },
        
        _ => -1,
    }
}

/// map from start to strat+len with permission port
pub fn mmap(start: usize, len: usize, port: usize)->isize {
    TASK_MANAGER.mmap(start, len, port)
}

/// the counter of map
pub fn munmap(start: usize, len: usize)->isize {
    TASK_MANAGER.munmap(start, len)
}