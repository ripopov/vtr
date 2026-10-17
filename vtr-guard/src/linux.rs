//! The guard on Linux.

use super::{Error, Options};
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64};
use vtr::{CrashState, Ending, Sealer, Writer};

const FATAL: [i32; 7] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGABRT,
    libc::SIGTRAP,
    libc::SIGSYS,
];
const STOP: [i32; 4] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGXCPU];
const ALTSTACK: usize = 64 << 10;

const IDLE: u32 = 0;
const RESCUING: u32 = 1;
const DONE: u32 = 2;

static STATE: AtomicU32 = AtomicU32::new(IDLE);
static SIGNO: AtomicI32 = AtomicI32::new(0);
static CODE: AtomicI32 = AtomicI32::new(0);
static ADDR: AtomicU64 = AtomicU64::new(0);
static TID: AtomicI32 = AtomicI32::new(0);
static EFD: AtomicI32 = AtomicI32::new(-1);
static DEADLINE_MS: AtomicU64 = AtomicU64::new(10_000);
static GRACE_MS: AtomicU64 = AtomicU64::new(10_000);
static STOP_SIG: AtomicI32 = AtomicI32::new(0);
static STOP_AT_MS: AtomicU64 = AtomicU64::new(0);
static STOP_ESCALATE: AtomicBool = AtomicBool::new(false);
static STOP_UNWATCHED: AtomicBool = AtomicBool::new(false);
static PARK_SIG: AtomicI32 = AtomicI32::new(0);
static INSTALLED: Mutex<Option<bool>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static TEST_STALL_MS: AtomicU64 = AtomicU64::new(0);
static TEST_FAULT: AtomicBool = AtomicBool::new(false);

struct OldActions(UnsafeCell<[MaybeUninit<libc::sigaction>; 65]>);
// SAFETY: Installation serializes writes before handlers are registered; handlers only read initialized signal entries.
unsafe impl Sync for OldActions {}
static OLD: OldActions = OldActions(UnsafeCell::new([const { MaybeUninit::uninit() }; 65]));

struct Slot {
    writer: *mut Writer,
    sealer: Sealer,
}

const SLOTS: usize = 64;
const RESERVED: *mut Slot = std::ptr::dangling_mut::<Slot>();
static WATCHED: [AtomicPtr<Slot>; SLOTS] = [const { AtomicPtr::new(null_mut()) }; SLOTS];
static OWNER: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];
static STATES: [AtomicPtr<CrashState>; SLOTS] = [const { AtomicPtr::new(null_mut()) }; SLOTS];
static PARKED: [AtomicBool; SLOTS] = [const { AtomicBool::new(false) }; SLOTS];

fn gettid() -> i32 {
    // SAFETY: gettid takes no pointer arguments.
    unsafe { libc::syscall(libc::SYS_gettid) as i32 }
}

fn now_ms() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a live, aligned output structure.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

fn say(s: &[u8]) {
    // SAFETY: The byte slice is readable for its full length; write does not retain it.
    unsafe { libc::write(2, s.as_ptr().cast(), s.len()) };
}

fn wake() {
    let one: u64 = 1;
    // SAFETY: one is a live eight-byte eventfd input; write does not retain it.
    unsafe { libc::write(EFD.load(Relaxed), (&raw const one).cast(), 8) };
}

unsafe fn restore(sig: i32) {
    // SAFETY: Only installed signals reach this function, and their prior actions were saved before registration.
    unsafe {
        let old = (*OLD.0.get())[sig as usize].as_ptr();
        libc::sigaction(sig, old, null_mut());
    }
}

fn tgkill(tid: i32, sig: i32) -> bool {
    // SAFETY: getpid and tgkill take only integer arguments.
    unsafe { libc::syscall(libc::SYS_tgkill, libc::getpid(), tid, sig) == 0 }
}

extern "C" fn on_fatal(sig: i32, info: *mut libc::siginfo_t, _: *mut c_void) {
    // SAFETY: libc provides a valid pointer to this thread’s errno.
    let errno = unsafe { *libc::__errno_location() };
    let tid = gettid();
    if STATE
        .compare_exchange(IDLE, RESCUING, AcqRel, Acquire)
        .is_err()
    {
        loop {
            // SAFETY: pause takes no pointers and is async-signal-safe.
            unsafe { libc::pause() };
        }
    }
    // SAFETY: SA_SIGINFO supplies a live siginfo_t for the duration of this handler.
    let (code, addr) = unsafe { ((*info).si_code, (*info).si_addr() as u64) };
    SIGNO.store(sig, Relaxed);
    CODE.store(code, Relaxed);
    ADDR.store(addr, Relaxed);
    TID.store(tid, Release);
    wake();
    let deadline = now_ms() + DEADLINE_MS.load(Relaxed);
    while STATE.load(Acquire) != DONE {
        if now_ms() >= deadline {
            say(b"vtr-guard: the rescue missed its deadline; the trace keeps what was written\n");
            break;
        }
        // SAFETY: A null pollfd array is valid when its element count is zero.
        unsafe { libc::poll(null_mut(), 0, 1) };
    }
    // SAFETY: The action for this installed signal is initialized, and errno belongs to this thread.
    unsafe {
        restore(sig);
        *libc::__errno_location() = errno;
    }
    let hardware = code > 0
        && matches!(
            sig,
            libc::SIGSEGV | libc::SIGBUS | libc::SIGILL | libc::SIGFPE
        );
    if !hardware {
        tgkill(tid, sig);
    }
}

extern "C" fn on_stop(sig: i32, _: *mut libc::siginfo_t, _: *mut c_void) {
    // SAFETY: libc provides a valid pointer to this thread’s errno.
    let errno = unsafe { *libc::__errno_location() };
    if STOP_SIG.compare_exchange(0, sig, AcqRel, Acquire).is_ok() {
        STOP_UNWATCHED.store(!any_watched(), Relaxed);
        STOP_AT_MS.store(now_ms(), Release);
    } else {
        STOP_ESCALATE.store(true, Release);
    }
    wake();
    // SAFETY: libc provides a valid pointer to this thread’s errno.
    unsafe { *libc::__errno_location() = errno };
}

fn park_here() -> ! {
    let tid = gettid();
    for i in 0..SLOTS {
        if OWNER[i].load(Relaxed) == tid {
            PARKED[i].store(true, Release);
        }
    }
    loop {
        // SAFETY: pause takes no pointers and is async-signal-safe.
        unsafe { libc::pause() };
    }
}

extern "C" fn on_park(_: i32, _: *mut libc::siginfo_t, _: *mut c_void) {
    let tid = gettid();
    for i in 0..SLOTS {
        if OWNER[i].load(Relaxed) != tid {
            continue;
        }
        let state = STATES[i].load(Acquire);
        // SAFETY: The owner thread cannot unwatch its writer while executing this signal handler.
        if !state.is_null() && unsafe { &*state }.busy() && !unsafe { &*state }.poisoned() {
            return;
        }
    }
    park_here();
}

fn park_on_leave() {
    park_here()
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

pub fn install(opts: &Options) -> Result<bool, Error> {
    let mut done = INSTALLED.lock().unwrap();
    if let Some(r) = *done {
        return Ok(r);
    }
    if std::env::var("VTR_GUARD").is_ok_and(|v| v.trim() == "0") {
        *done = Some(false);
        return Ok(false);
    }
    DEADLINE_MS.store(
        env_u64("VTR_GUARD_DEADLINE_MS").unwrap_or(opts.deadline.as_millis() as u64),
        Relaxed,
    );
    GRACE_MS.store(
        env_u64("VTR_GUARD_STOP_GRACE_MS").unwrap_or(opts.stop_grace.as_millis() as u64),
        Relaxed,
    );
    TEST_STALL_MS.store(env_u64("VTR_GUARD_TEST_STALL_MS").unwrap_or(0), Relaxed);
    TEST_FAULT.store(env_u64("VTR_GUARD_TEST_RESCUE_FAULT") == Some(1), Relaxed);
    let park = if opts.park_signal != 0 {
        opts.park_signal
    } else {
        libc::SIGRTMAX() - 3
    };
    PARK_SIG.store(park, Relaxed);
    // SAFETY: The installation mutex serializes registration; stack structures stay live for each syscall.
    unsafe {
        let efd = libc::eventfd(0, libc::EFD_CLOEXEC);
        if efd < 0 {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }
        EFD.store(efd, Relaxed);
        let mut all = MaybeUninit::<libc::sigset_t>::uninit();
        let mut prev = MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigfillset(all.as_mut_ptr());
        libc::pthread_sigmask(libc::SIG_SETMASK, all.as_ptr(), prev.as_mut_ptr());
        let spawned = std::thread::Builder::new()
            .name("vtr-guard".into())
            .spawn(rescue_main);
        libc::pthread_sigmask(libc::SIG_SETMASK, prev.as_ptr(), null_mut());
        spawned.map_err(Error::Os)?;
        altstack()?;
        let mut sa: libc::sigaction = std::mem::zeroed();
        libc::sigfillset(&raw mut sa.sa_mask);
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        sa.sa_sigaction = on_park as *const () as usize;
        libc::sigaction(park, &raw const sa, null_mut());
        let old = &mut *OLD.0.get();
        if opts.crashes {
            sa.sa_sigaction = on_fatal as *const () as usize;
            for sig in FATAL {
                libc::sigaction(sig, null_mut(), old[sig as usize].as_mut_ptr());
                libc::sigaction(sig, &raw const sa, null_mut());
            }
        }
        if opts.stops {
            sa.sa_sigaction = on_stop as *const () as usize;
            sa.sa_flags |= libc::SA_RESTART;
            for sig in STOP {
                libc::sigaction(sig, null_mut(), old[sig as usize].as_mut_ptr());
                if old[sig as usize].assume_init_ref().sa_sigaction == libc::SIG_DFL {
                    libc::sigaction(sig, &raw const sa, null_mut());
                }
            }
        }
        if opts.exit {
            on_exit(at_exit, null_mut());
        }
    }
    ACTIVE.store(true, Release);
    *done = Some(true);
    Ok(true)
}

unsafe extern "C" {
    fn on_exit(f: extern "C" fn(i32, *mut c_void), arg: *mut c_void) -> i32;
}

pub fn thread_init() {
    drop(altstack());
}

type StopCallback = Box<dyn Fn(i32) + Send + Sync>;
static STOP_CALLBACKS: Mutex<Vec<(u64, StopCallback)>> = Mutex::new(Vec::new());
static NEXT_CALLBACK: AtomicU64 = AtomicU64::new(1);

pub fn add_stop_callback(f: StopCallback) -> u64 {
    let id = NEXT_CALLBACK.fetch_add(1, Relaxed);
    STOP_CALLBACKS.lock().unwrap().push((id, f));
    id
}

pub fn remove_stop_callback(id: u64) {
    STOP_CALLBACKS.lock().unwrap().retain(|(i, _)| *i != id);
}

fn altstack() -> Result<(), Error> {
    // SAFETY: The mapped stack remains allocated for the thread lifetime; guard-page arithmetic stays within it.
    unsafe {
        let mut cur: libc::stack_t = std::mem::zeroed();
        libc::sigaltstack(null_mut(), &raw mut cur);
        if cur.ss_flags & libc::SS_DISABLE == 0 && cur.ss_size >= 16 << 10 {
            return Ok(());
        }
        let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        let p = libc::mmap(
            null_mut(),
            ALTSTACK + page,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }
        libc::mprotect(p, page, libc::PROT_NONE);
        let ss = libc::stack_t {
            ss_sp: p.cast::<u8>().add(page).cast(),
            ss_flags: 0,
            ss_size: ALTSTACK,
        };
        if libc::sigaltstack(&raw const ss, null_mut()) != 0 {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

/// # Safety
/// The writer must remain live at a stable address on this thread until unwatched.
/// Its operations must participate in the crash-state guard so rescue can wait
/// for exclusive access before closing it.
pub unsafe fn watch(writer: *mut Writer) -> Result<(), Error> {
    // SAFETY: The caller keeps writer live and immovable; the slot owns the sealer before publishing its state.
    unsafe {
        if !ACTIVE.load(Acquire) {
            return Ok(());
        }
        altstack()?;
        let sealer = (*writer).sealer();
        sealer.state().set_watched(true);
        for i in 0..SLOTS {
            if WATCHED[i]
                .compare_exchange(null_mut(), RESERVED, AcqRel, Relaxed)
                .is_ok()
            {
                OWNER[i].store(gettid(), Relaxed);
                PARKED[i].store(false, Relaxed);
                let slot = Box::into_raw(Box::new(Slot { writer, sealer }));
                STATES[i].store(
                    std::ptr::from_ref::<CrashState>((*slot).sealer.state()).cast_mut(),
                    Release,
                );
                WATCHED[i].store(slot, Release);
                return Ok(());
            }
        }
        Err(Error::Full)
    }
}

pub fn unwatch(writer: *mut Writer) {
    for i in 0..SLOTS {
        let p = WATCHED[i].load(Acquire);
        if p.is_null() || p == RESERVED {
            continue;
        }
        // SAFETY: Published slots stay live until the successful removal below; registration and removal belong to their owner.
        if unsafe { (*p).writer } == writer
            && WATCHED[i]
                .compare_exchange(p, null_mut(), AcqRel, Relaxed)
                .is_ok()
        {
            STATES[i].store(null_mut(), Release);
            OWNER[i].store(0, Relaxed);
            // SAFETY: The successful compare_exchange claims the Box allocated by watch exactly once.
            let slot = unsafe { Box::from_raw(p) };
            slot.sealer.state().set_watched(false);
        }
    }
}

pub fn stop_requested() -> i32 {
    STOP_SIG.load(Acquire)
}

#[must_use]
pub fn ending() -> Ending {
    match stop_requested() {
        0 => Ending::Closed,
        signal => Ending::Stopped { signal },
    }
}

fn rescue_main() {
    warm_up();
    let mut told = false;
    loop {
        let stop = STOP_SIG.load(Acquire);
        let timeout = if stop != 0 {
            let at = STOP_AT_MS.load(Acquire) + GRACE_MS.load(Relaxed);
            at.saturating_sub(now_ms()).min(i32::MAX as u64) as i32
        } else {
            -1
        };
        let mut pfd = libc::pollfd {
            fd: EFD.load(Relaxed),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is a live array of one initialized pollfd.
        if unsafe { libc::poll(&raw mut pfd, 1, timeout) } > 0 {
            let mut v = 0u64;
            // SAFETY: v is a writable eight-byte eventfd output; read does not retain it.
            unsafe { libc::read(pfd.fd, (&raw mut v).cast(), 8) };
        }
        if STATE.load(Acquire) == RESCUING && TID.load(Acquire) != 0 {
            crash_rescue();
            STATE.store(DONE, Release);
            loop {
                // SAFETY: pause takes no pointers and is async-signal-safe.
                unsafe { libc::pause() };
            }
        }
        let stop = STOP_SIG.load(Acquire);
        if stop != 0 && !told {
            told = true;
            for (_, f) in STOP_CALLBACKS.lock().unwrap().iter() {
                f(stop);
            }
        }
        if stop != 0
            && (STOP_ESCALATE.load(Acquire)
                || now_ms() >= STOP_AT_MS.load(Acquire) + GRACE_MS.load(Relaxed)
                || STOP_UNWATCHED.load(Relaxed))
        {
            stop_rescue(stop);
        }
    }
}

fn warm_up() {
    let (tx, rx) = std::sync::mpsc::sync_channel::<u8>(1);
    tx.send(0).unwrap();
    rx.recv().unwrap();
    drop(std::thread::current());
    drop(format!("{}", now_ms()));
}

fn any_watched() -> bool {
    WATCHED.iter().any(|w| !w.load(Acquire).is_null())
}

fn is_vtr_thread(tid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/self/task/{tid}/comm"))
        .is_ok_and(|c| c.starts_with("vtr-"))
}

fn crash_rescue() {
    let t0 = now_ms();
    let (signal, code, address, thread) = (
        SIGNO.load(Relaxed),
        CODE.load(Relaxed),
        ADDR.load(Relaxed),
        TID.load(Acquire),
    );
    let name = vtr::ending::signal_name(signal).unwrap_or("a signal");
    if is_vtr_thread(thread) {
        say(format!(
            "vtr-guard: {name} on VTR's own thread {thread}; the traces keep what was written\n"
        )
        .as_bytes());
        return;
    }
    let stall = TEST_STALL_MS.load(Relaxed);
    if stall > 0 {
        // SAFETY: A null pollfd array is valid when its element count is zero.
        unsafe { libc::poll(null_mut(), 0, stall as i32) };
    }
    if TEST_FAULT.load(Relaxed) {
        // Rescue blocks signals; fault injection must restore default delivery before raising SIGSEGV.
        // SAFETY: The mask is initialized before use, and SIG_DFL is a valid signal disposition.
        unsafe {
            libc::signal(libc::SIGSEGV, libc::SIG_DFL);
            let mut set = MaybeUninit::<libc::sigset_t>::uninit();
            libc::sigemptyset(set.as_mut_ptr());
            libc::sigaddset(set.as_mut_ptr(), libc::SIGSEGV);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, set.as_ptr(), null_mut());
            libc::raise(libc::SIGSEGV);
        }
        unreachable!("SIGSEGV terminates rescue fault injection");
    }
    let n = finish_all(
        Ending::Crashed {
            signal,
            code,
            address,
            thread: thread as u64,
            sealed: false,
        },
        thread,
    );
    say(format!(
        "vtr-guard: {name} in thread {thread}; {n} trace(s) finished in {} ms\n",
        now_ms() - t0
    )
    .as_bytes());
}

fn stop_rescue(signal: i32) -> ! {
    if STATE
        .compare_exchange(IDLE, RESCUING, AcqRel, Acquire)
        .is_ok()
    {
        finish_all(Ending::Stopped { signal }, 0);
    }
    // SAFETY: The installed signal has a saved action; sigemptyset initializes set before it is read.
    unsafe {
        restore(signal);
        let mut set = MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        libc::sigaddset(set.as_mut_ptr(), signal);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, set.as_ptr(), null_mut());
        libc::raise(signal);
    }
    unreachable!("the stop signal ends the process")
}

extern "C" fn at_exit(status: i32, _: *mut c_void) {
    if any_watched()
        && STATE
            .compare_exchange(IDLE, RESCUING, AcqRel, Acquire)
            .is_ok()
    {
        let end = match stop_requested() {
            0 => Ending::Exited { status },
            signal => Ending::Stopped { signal },
        };
        finish_all(end, gettid());
        STATE.store(IDLE, Release);
    }
    let stop = stop_requested();
    if stop != 0 {
        // SAFETY: The stop signal has an initialized saved action and raise takes no pointer arguments.
        unsafe {
            restore(stop);
            libc::raise(stop);
        }
    }
}

fn finish_all(end: Ending, stopped: i32) -> usize {
    let me = gettid();
    let park = PARK_SIG.load(Relaxed);
    let mut waiting = [false; SLOTS];
    for i in 0..SLOTS {
        let state = STATES[i].load(Acquire);
        let owner = OWNER[i].load(Relaxed);
        if state.is_null() || owner == 0 || owner == stopped || owner == me {
            continue;
        }
        // SAFETY: Published crash states are retained by watched slots throughout rescue.
        unsafe { &*state }.request_park(park_on_leave);
        waiting[i] = tgkill(owner, park);
    }
    let until = now_ms() + DEADLINE_MS.load(Relaxed) / 4;
    while (0..SLOTS).any(|i| waiting[i] && !PARKED[i].load(Acquire)) && now_ms() < until {
        // SAFETY: A null pollfd array is valid when its element count is zero.
        unsafe { libc::poll(null_mut(), 0, 1) };
    }
    let mut n = 0;
    for i in 0..SLOTS {
        let p = WATCHED[i].swap(null_mut(), AcqRel);
        if p.is_null() || p == RESERVED {
            continue;
        }
        // SAFETY: The atomic swap claims a published slot; rescue retains its allocation.
        let slot = unsafe { &*p };
        let owner = OWNER[i].load(Relaxed);
        let stopped_here =
            owner == stopped || owner == me || PARKED[i].load(Acquire) || !waiting[i];
        let state = slot.sealer.state();
        let r = if stopped_here && !state.busy() {
            // SAFETY: The owner is stopped and the crash state is idle, giving rescue exclusive access.
            unsafe { (*slot.writer).close_with(end) }
        } else {
            let end = match end {
                Ending::Crashed {
                    signal,
                    code,
                    address,
                    thread,
                    ..
                } => Ending::Crashed {
                    signal,
                    code,
                    address,
                    thread,
                    sealed: true,
                },
                e => e,
            };
            slot.sealer.seal(end)
        };
        match r {
            Ok(()) => n += 1,
            Err(e) => say(format!("vtr-guard: could not finish a trace: {e}\n").as_bytes()),
        }
    }
    n
}
