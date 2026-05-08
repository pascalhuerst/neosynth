use libc::{
    CPU_ISSET, CPU_SET, CPU_SETSIZE, CPU_ZERO, SCHED_BATCH, SCHED_FIFO, SCHED_IDLE, SCHED_OTHER,
    SCHED_RR, cpu_set_t, sched_getaffinity, sched_getparam, sched_getscheduler, sched_param,
    sched_setaffinity, sched_setscheduler,
};

/// SCHED_FIFO priority for the audio thread. 80 matches legacy xwax.
const RT_PRIORITY: i32 = 80;

/// Promote the calling thread to SCHED_FIFO and log whether it actually took.
///
/// Note: both `sched_setscheduler` and `sched_setaffinity` take a *TID*, not a
/// PID. Passing `0` means "the calling thread", which is what we want here —
/// we run from the spawned audio thread. (`getpid()` would target the main
/// thread on Linux, which was the prior bug.)
pub fn prioritize_thread() {
    let param = sched_param {
        sched_priority: RT_PRIORITY,
    };
    let set_rc = unsafe { sched_setscheduler(0, SCHED_FIFO, &param) };
    if set_rc != 0 {
        tracing::warn!(
            "sched_setscheduler(SCHED_FIFO, prio={}) failed: {}. Try `sudo setcap 'cap_sys_nice=eip' <binary>` or raise the rtprio ulimit.",
            RT_PRIORITY,
            std::io::Error::last_os_error(),
        );
    }

    // Read back so the operator can confirm what the kernel actually granted.
    let policy = unsafe { sched_getscheduler(0) };
    let mut got = sched_param { sched_priority: 0 };
    let _ = unsafe { sched_getparam(0, &mut got) };

    let policy_name = match policy {
        SCHED_FIFO => "SCHED_FIFO",
        SCHED_RR => "SCHED_RR",
        SCHED_OTHER => "SCHED_OTHER",
        SCHED_BATCH => "SCHED_BATCH",
        SCHED_IDLE => "SCHED_IDLE",
        _ => "UNKNOWN",
    };
    let realtime = matches!(policy, SCHED_FIFO | SCHED_RR);
    if realtime {
        tracing::info!(
            "Audio thread scheduling: {} priority {} (realtime: yes)",
            policy_name,
            got.sched_priority,
        );
    } else {
        tracing::warn!(
            "Audio thread scheduling: {} priority {} (realtime: NO — expect xruns under load)",
            policy_name,
            got.sched_priority,
        );
    }
}

/// Pin the calling thread to a specific CPU core, then log the resulting mask.
pub fn set_thread_affinity(core_id: usize) {
    let mut set: cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe {
        CPU_ZERO(&mut set);
        CPU_SET(core_id, &mut set);
    }

    let set_rc = unsafe { sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &set) };
    if set_rc != 0 {
        tracing::warn!(
            "sched_setaffinity(cpu={}) failed: {}",
            core_id,
            std::io::Error::last_os_error(),
        );
    }

    // Read back the mask the kernel actually applied.
    let mut got: cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe { CPU_ZERO(&mut got) };
    let get_rc =
        unsafe { sched_getaffinity(0, std::mem::size_of::<cpu_set_t>(), &mut got) };
    if get_rc != 0 {
        tracing::warn!(
            "sched_getaffinity failed: {}",
            std::io::Error::last_os_error(),
        );
        return;
    }

    let mut cpus: Vec<usize> = Vec::new();
    for i in 0..(CPU_SETSIZE as usize) {
        if unsafe { CPU_ISSET(i, &got) } {
            cpus.push(i);
        }
    }

    let pinned = cpus.len() == 1 && cpus.first() == Some(&core_id);
    if pinned {
        tracing::info!("Audio thread pinned to CPU {} (affinity: yes)", core_id);
    } else {
        tracing::warn!(
            "Audio thread affinity: cpus {:?} (requested CPU {} only — pinning: NO)",
            cpus,
            core_id,
        );
    }
}
