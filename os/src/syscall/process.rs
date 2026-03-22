//! Process management syscalls
use crate::{mm::{translated_byte_buffer}, task::{change_program_brk, current_user_token, exit_current_and_run_next, mmap, munmap, suspend_current_and_run_next}, timer::get_time_us };

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let usec=get_time_us();
    let time_val=TimeVal {
        sec:usec/1_000_000,
        usec:usec%1_000_000,
    };
    let len=core::mem::size_of::<TimeVal>();
    let token=current_user_token();
    let buffers=translated_byte_buffer(token, ts as *const u8, len);
    let mut start=0;
    let data=unsafe {
        core::slice::from_raw_parts(&time_val as *const _ as *const u8 , len)
    };
    for buffer in buffers {
        let len=buffer.len();
        buffer.copy_from_slice(&data[start..start+len]);
        start+=len;
    }
    0
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    crate::task::task_sys_trace(trace_request, id, data)
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap NOT IMPLEMENTED YET!");
    mmap(start, len, port)
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    munmap(start, len)
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
