//! The profiler on threads whose work is known. No graphics card is used.

use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

fn config(hz: u32) -> Config {
    Config::new(hz, std::env::temp_dir().join("victory_profile_test.txt"))
}

/// Starts a thread running `work` and returns its id once it runs.
fn thread(work: impl FnOnce() + Send + 'static) -> (u32, std::thread::JoinHandle<()>) {
    let (said, hears) = mpsc::channel();
    let running = std::thread::spawn(move || {
        // SAFETY: a plain query.
        said.send(unsafe { GetCurrentThreadId() }).unwrap();
        work();
    });
    (hears.recv().unwrap(), running)
}

/// Looks at the followed threads for `time`, a look every `period`, with
/// `frames` presents spread over it.
fn sample(sampler: &mut Sampler, time: Duration, period: Duration, frames: &AtomicU64, each: u64) {
    sampler.resync();
    let began = Instant::now();
    let mut looks = 0u64;
    while began.elapsed() < time {
        std::thread::sleep(period);
        sampler.tick();
        looks += 1;
        if each > 0 && looks.is_multiple_of(each) {
            frames.fetch_add(1, Relaxed);
        }
    }
}

#[inline(never)]
fn profile_test_spin_inner(stop: &AtomicBool) -> u64 {
    let mut sum = 0u64;
    while !stop.load(Relaxed) {
        for i in 0..2000u64 {
            sum = std::hint::black_box(sum.wrapping_mul(31).wrapping_add(i));
        }
    }
    sum
}

#[inline(never)]
fn profile_test_spins(stop: &AtomicBool) -> u64 {
    std::hint::black_box(profile_test_spin_inner(stop)).wrapping_add(1)
}

#[inline(never)]
fn profile_test_sleeps(stop: &AtomicBool) -> u64 {
    let mut naps = 0;
    while !stop.load(Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
        naps = std::hint::black_box(naps + 1);
    }
    naps
}

#[inline(never)]
fn profile_test_locks(lock: &Mutex<u64>) -> u64 {
    let mut held = lock.lock().unwrap();
    *held += 1;
    std::hint::black_box(*held)
}

#[inline(never)]
fn profile_test_deep(left: u32, stop: &AtomicBool) -> u64 {
    // A frame of a kilobyte, so a hundred of them pass any small limit.
    let mut room = [0u64; 128];
    room[left as usize % 128] = u64::from(left);
    let room = std::hint::black_box(room);
    let below = if left == 0 {
        profile_test_sleeps(stop)
    } else {
        profile_test_deep(left - 1, stop)
    };
    std::hint::black_box(below.wrapping_add(room[3]))
}

/// Whether `address` is committed code.
fn executable(address: u64) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    // SAFETY: a zeroed MEMORY_BASIC_INFORMATION is valid; the query reads
    // the address space's bookkeeping, not `address`.
    unsafe {
        let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        VirtualQuery(address as *const _, &mut info, size_of_val(&info)) != 0
            && info.State == MEM_COMMIT
            && info.Protect & 0xf0 != 0
            && info.Protect & 0x100 == 0
    }
}

fn own(role: &RoleSummary, what: &str) -> u64 {
    role.functions
        .iter()
        .filter(|f| f.0.contains(what))
        .map(|f| f.1)
        .sum()
}

fn with(role: &RoleSummary, what: &str) -> u64 {
    role.functions
        .iter()
        .filter(|f| f.0.contains(what))
        .map(|f| f.2)
        .max()
        .unwrap_or(0)
}

fn role(report: &Report, role: Role) -> &RoleSummary {
    report
        .roles
        .iter()
        .find(|r| r.role == role)
        .unwrap_or_else(|| panic!("no {role:?} in\n{}", report.text))
}

/// A busy loop in one function, a sleep in another, a wait for a lock in
/// a third: the report gives each thread's time to the right one, as
/// running or as waiting, and per frame.
#[test]
fn time_goes_to_the_function_that_spent_it() {
    static PRESENTS: AtomicU64 = AtomicU64::new(0);
    let stop = Arc::new(AtomicBool::new(false));
    let lock = Arc::new(Mutex::new(0u64));
    let held = lock.lock().unwrap();
    let (spinning, a) = thread({
        let stop = stop.clone();
        move || {
            std::hint::black_box(profile_test_spins(&stop));
        }
    });
    let (sleeping, b) = thread({
        let stop = stop.clone();
        move || {
            std::hint::black_box(profile_test_sleeps(&stop));
        }
    });
    let (locked, c) = thread({
        let lock = lock.clone();
        move || {
            std::hint::black_box(profile_test_locks(&lock));
        }
    });
    std::thread::sleep(Duration::from_millis(50));

    let mut sampler = Sampler::new(config(1000));
    sampler.presents = &PRESENTS;
    sampler.follow(spinning, Role::Submit, null_mut(), Some("spins".into()));
    sampler.follow(sleeping, Role::Record, null_mut(), Some("sleeps".into()));
    sampler.follow(locked, Role::Wide, null_mut(), Some("locks".into()));
    assert_eq!(sampler.threads.len(), 3);
    assert!(
        sampler.threads.iter().all(|t| t.low != 0 && t.low < t.high),
        "stack bounds"
    );
    sample(
        &mut sampler,
        Duration::from_millis(1500),
        Duration::from_millis(1),
        &PRESENTS,
        20,
    );
    let report = sampler.report();
    stop.store(true, Relaxed);
    drop(held);
    for running in [a, b, c] {
        running.join().unwrap();
    }
    println!("{}", report.text);
    assert!(sampler.presented >= 20, "{} frames", sampler.presented);
    assert!(
        sampler.symbols.as_ref().is_some_and(|s| s.pdb.0),
        "the test's PDB was not found"
    );

    // The busy loop: all of it in the inner function, called from the outer.
    let spins = role(&report, Role::Submit);
    assert!(spins.wall >= 1_200_000, "{} us sampled", spins.wall);
    // (An unoptimised build calls the loop's own pieces; an optimised one
    // has them inside it.)
    let inner = with(spins, "profile_test_spin_inner");
    assert!(
        inner * 100 >= spins.wall * 95,
        "inner {inner} of {}",
        spins.wall
    );
    assert!(with(spins, "profile_test_spins") * 100 >= spins.wall * 95);
    assert!(own(spins, "profile_test_spins") * 100 <= spins.wall * 5);
    assert!(
        spins.waiting * 100 <= spins.wall * 5,
        "waiting {}",
        spins.waiting
    );
    // Every sample is some function's own time, once.
    assert_eq!(spins.functions.iter().map(|f| f.1).sum::<u64>(), spins.wall);

    // The sleep: waiting, for the function that slept.
    let sleeps = role(&report, Role::Record);
    assert!(
        sleeps.waiting * 100 >= sleeps.wall * 85,
        "{} of {}",
        sleeps.waiting,
        sleeps.wall
    );
    let (kind, caller, time) = &sleeps.waits[0];
    assert!(
        kind == "sleep" && caller.contains("profile_test_sleeps"),
        "{kind}, {caller}"
    );
    assert!(time * 100 >= sleeps.waiting * 95);

    // The lock: waiting, for the function that wanted it.
    let locks = role(&report, Role::Wide);
    assert!(
        locks.waiting * 100 >= locks.wall * 95,
        "{} of {}",
        locks.waiting,
        locks.wall
    );
    let (kind, caller, _) = &locks.waits[0];
    assert!(
        kind == "lock (ours)" && caller.contains("profile_test_locks"),
        "{kind}, {caller}"
    );

    // Per frame, and the stacks for a flame graph: outer before inner.
    assert!(report.text.contains("ms a frame"));
    let stack = report
        .folded
        .lines()
        .find(|l| l.starts_with("submit;") && l.contains("profile_test_spin_inner"))
        .unwrap_or_else(|| panic!("{}", report.folded));
    let (outer, inner) = (
        stack.find("profile_test_spins").unwrap(),
        stack.find("profile_test_spin_inner").unwrap(),
    );
    assert!(outer < inner, "{stack}");
    // The thread that waited the whole time was suspended once at most a
    // few times: it had not run since the look before.
    let waiting = sampler.threads.iter().find(|t| t.id == locked).unwrap();
    assert!(
        waiting.reused * 10 >= waiting.looks * 9,
        "{} of {}",
        waiting.reused,
        waiting.looks
    );
}

/// A stack deeper than is kept, and one longer than is copied: the
/// innermost frames are still named, and the stack is marked as cut.
#[test]
fn deep_stacks_are_cut_not_lost() {
    let stop = Arc::new(AtomicBool::new(false));
    let (deep, running) = thread({
        let stop = stop.clone();
        move || {
            std::hint::black_box(profile_test_deep(150, &stop));
        }
    });
    std::thread::sleep(Duration::from_millis(50));
    for (depth, stack) in [(24, 128 << 10), (MAX_DEPTH, 8 << 10)] {
        static PRESENTS: AtomicU64 = AtomicU64::new(0);
        let mut settings = config(500);
        (settings.depth, settings.stack, settings.reuse) = (depth, stack, false);
        let mut sampler = Sampler::new(settings);
        sampler.presents = &PRESENTS;
        sampler.follow(deep, Role::Busy, null_mut(), None);
        sample(
            &mut sampler,
            Duration::from_millis(300),
            Duration::from_millis(2),
            &PRESENTS,
            0,
        );
        let report = sampler.report();
        let thread = &sampler.threads[0];
        assert!(
            thread.looks > 20 && thread.missed == 0,
            "{} looks",
            thread.looks
        );
        assert_eq!(
            thread.cut, thread.looks,
            "depth {depth}, {stack} bytes copied"
        );
        let busy = role(&report, Role::Busy);
        assert!(
            with(busy, "[frames not kept]") * 100 >= busy.wall * 99,
            "{}",
            report.text
        );
        assert!(
            with(busy, "profile_test_sleeps") * 100 >= busy.wall * 90,
            "{}",
            report.text
        );
        assert!(report.text.contains("ms a second"));
    }
    stop.store(true, Relaxed);
    running.join().unwrap();
}

/// Registers and stack bytes that mean nothing, at addresses inside real
/// code: the walk stops, and reads nothing outside its copy (a read
/// elsewhere would end the test with an access violation).
#[test]
fn stacks_that_mean_nothing_are_walked_without_a_fault() {
    let mut symbols = Symbols::new(false);
    symbols.refresh();
    let modules: Vec<(u64, u64)> = symbols.modules.iter().map(|m| (m.base, m.end)).collect();
    assert!(modules.len() > 3);
    let mut sampler = Sampler::new(config(1000));
    // SAFETY: a plain query.
    let me = unsafe { GetCurrentThreadId() };
    // A thread entry is all the walk needs; this one is never suspended.
    sampler.follow(me, Role::Busy, null_mut(), None);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // Return instructions (bytes that read as one) in the modules' code.
    let mut returns = Vec::new();
    while returns.len() < 20_000 {
        let r = next();
        let (base, end) = modules[(r >> 32) as usize % modules.len()];
        let at = base + 0x1000 + r % (end - base - 0x1000).max(1);
        if !executable(at) {
            continue;
        }
        // SAFETY: committed code of a loaded module, as just asked, to
        // the end of its page.
        let bytes =
            unsafe { std::slice::from_raw_parts(at as *const u8, 0x1000 - (at as usize & 0xfff)) };
        if let Some(ret) = bytes.iter().skip(16).position(|&b| b == 0xc3) {
            returns.push(at + 16 + ret as u64);
        }
    }
    let limit = sampler.config.stack;
    let (mut walked, mut frames) = (0u64, 0u64);
    let rounds = env_number("VICTORY_PROFILE_TEST_WALKS").map_or(60_000, |n| n as u64);
    for round in 0..rounds {
        let got = (next() as usize % limit) & !7;
        // A pretended stack somewhere, or the thread's own range.
        let rsp = if round.is_multiple_of(2) {
            (0x7000_0000_0000u64 + (next() & 0xffff_fff0)) as usize
        } else {
            sampler.threads[0].low + 0x1000 + (next() as usize & 0xff8)
        };
        let code = |r: u64| {
            if r.is_multiple_of(3) {
                // A few bytes before a return: where functions end.
                return returns[(r >> 32) as usize % returns.len()] - (r >> 8 & 15);
            }
            let (base, end) = modules[(r >> 32) as usize % modules.len()];
            base + 0x1000 + r % (end - base - 0x1000).max(1)
        };
        // The copy: words that are nothing, stack addresses, code addresses.
        for at in (0..got).step_by(8) {
            let r = next();
            let word = match r % 4 {
                0 => r,
                1 => (rsp + (next() as usize % (got + 4096))) as u64,
                2 => code(next()),
                _ => r & 0xffff,
            };
            // SAFETY: inside the copy.
            unsafe {
                sampler
                    .copy
                    .add((rsp & 15) + at)
                    .cast::<u64>()
                    .write_unaligned(word)
            };
        }
        let end = (rsp & 15) + got;
        if end < sampler.dirty {
            // SAFETY: inside the copy.
            unsafe { sampler.copy.add(end).write_bytes(0, sampler.dirty - end) };
        }
        sampler.dirty = end;
        let context = &mut sampler.context.0;
        for register in [
            &mut context.Rax,
            &mut context.Rbx,
            &mut context.Rcx,
            &mut context.Rdx,
            &mut context.Rbp,
            &mut context.Rsi,
            &mut context.Rdi,
            &mut context.R8,
            &mut context.R9,
            &mut context.R10,
            &mut context.R11,
            &mut context.R12,
            &mut context.R13,
            &mut context.R14,
            &mut context.R15,
        ] {
            let r = next();
            *register = match r % 3 {
                0 => r,
                1 => (rsp + (next() as usize % (got + 4096))) as u64,
                _ => r & 0xfff,
            };
        }
        context.Rsp = rsp as u64;
        context.Rip = code(next());
        let (count, _, used) = sampler.walk(0, got);
        assert!(used <= got + 8, "{used} of {got}");
        walked += 1;
        frames += count as u64;
    }
    println!("{walked} walks of stacks that mean nothing, {frames} frames");
    assert!(frames > walked);
}

/// The walk of another thread's copied stack names the same callers as
/// the system's own walk of that thread by itself.
#[test]
fn the_walk_in_the_copy_is_the_systems_walk() {
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    let (said, hears) = mpsc::channel();
    let (go, wait) = mpsc::channel::<()>();
    #[inline(never)]
    fn profile_test_parked(said: &mpsc::Sender<Vec<u64>>, wait: &mpsc::Receiver<()>) -> u64 {
        let mut own = [null_mut(); 32];
        // SAFETY: writes at most 32 return addresses into `own`.
        let n = unsafe { RtlCaptureStackBackTrace(0, 32, own.as_mut_ptr(), null_mut()) };
        said.send(own[..usize::from(n)].iter().map(|&a| a as u64).collect())
            .unwrap();
        wait.recv().ok();
        std::hint::black_box(u64::from(n))
    }
    let (parked, running) = thread(move || {
        std::hint::black_box(profile_test_parked(&said, &wait));
    });
    let own = hears.recv().unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let mut sampler = Sampler::new(config(1000));
    sampler.follow(parked, Role::Busy, null_mut(), None);
    let (got, _) = sampler.capture(0).expect("the thread is looked at");
    let (frames, whole, used) = sampler.walk(0, got);
    assert!(used > 64 && used <= got, "{used} of {got} bytes");
    let walked: Vec<u64> = sampler.frames[..frames].to_vec();
    go.send(()).unwrap();
    running.join().unwrap();
    assert!(whole, "{frames} frames");
    // The system's walk starts in the function that asked; ours deeper, in
    // the wait. From that function's caller on they are the same calls
    // (ours keeps the byte before each return address).
    let callers: Vec<u64> = own[1..].iter().map(|a| a - 1).collect();
    assert!(callers.len() >= 3, "{own:x?}");
    let at = walked
        .iter()
        .position(|&a| a == callers[0])
        .unwrap_or_else(|| panic!("{walked:x?} {callers:x?}"));
    assert_eq!(
        &walked[at..],
        &callers[..],
        "ours {walked:x?}, the system's {own:x?}"
    );
}

#[test]
fn names_say_what_waits_and_whose_code_it_is() {
    assert!(waits("ntdll!NtWaitForSingleObject"));
    assert!(waits("ntdll!ZwWaitForAlertByThreadId"));
    assert!(waits("ntdll!NtDelayExecution"));
    assert!(waits(
        "win32u!NtGdiDdDDIWaitForSynchronizationObjectFromCpu"
    ));
    assert!(!waits("ntdll!NtQueryVirtualMemory"));
    assert!(!waits("ntdll!NtYieldExecution"));
    assert!(!waits("victory_gpu::host::Host::submit"));
    assert_eq!(
        wait_kind(&[
            "ntdll!NtWaitForAlertByThreadId",
            "ntdll!RtlAcquireSRWLockExclusive"
        ]),
        "lock (SRW)"
    );
    assert_eq!(
        wait_kind(&[
            "ntdll!NtWaitForAlertByThreadId",
            "ntdll.dll+0x1230",
            "std::sys::sync::mutex::futex::Mutex::lock_contended"
        ]),
        "lock (ours)"
    );
    assert_eq!(
        wait_kind(&[
            "ntdll!NtWaitForAlertByThreadId",
            "std::sys::sync::condvar::futex::Condvar::wait"
        ]),
        "condition variable"
    );
    assert_eq!(
        wait_kind(&[
            "ntdll!NtWaitForSingleObject",
            "KERNELBASE!WaitForSingleObjectEx"
        ]),
        "event or handle"
    );
    assert_eq!(
        wait_kind(&["ntdll!NtDelayExecution", "KERNELBASE!SleepEx"]),
        "sleep"
    );
    assert_eq!(kind_of("nvwgf2umx.dll", false), Kind::Driver);
    assert_eq!(kind_of("ntdll.dll", false), Kind::System);
    assert_eq!(kind_of("d3d12core.dll", false), Kind::Direct3d);
    assert_eq!(
        kind_of("victory-image-0123456789abcdef.dll", false),
        Kind::Title
    );
    assert_eq!(kind_of("vcruntime140.dll", false), Kind::Other);
    assert_eq!(kind_of("victory.exe", true), Kind::Ours);
    assert_eq!(Role::of_name("victory-pipelines-3"), Some(Role::Pipelines));
    assert_eq!(Role::of_name("victory-wide-0"), Some(Role::Wide));
    assert_eq!(Role::of_name("victory-window"), None);
}

#[test]
fn times_are_classed_four_to_a_power_of_two() {
    assert_eq!(held_class(0), (0, 0));
    assert_eq!(held_class(3), (3, 3));
    assert_eq!(held_class(4), (8, 4));
    assert_eq!(held_class(7), (11, 7));
    assert_eq!(held_class(8), (12, 8));
    assert_eq!(held_class(9), (12, 8));
    assert_eq!(held_class(10), (13, 10));
    assert_eq!(held_class(20_000), (56, 16_384));
    let mut sampler = Sampler::new(config(1000));
    for ns in [1000, 20_000, 20_000, 20_000, 500_000] {
        sampler.held += 1;
        sampler.held_by[held_class(ns).0] += 1;
        sampler.held_longest = sampler.held_longest.max(ns);
    }
    // The middle one of five is a 20 us one: its class ends at 20.48.
    assert_eq!(sampler.held_within(0.5), 20.48);
    assert_eq!(sampler.held_within(0.99), 524.288);
}

#[test]
fn a_tree_keeps_each_call_site_once() {
    let mut tree = Tree::new();
    // Innermost first: 3 called by 2 called by 1.
    let a = tree.place(&[3, 2, 1], true, true);
    let b = tree.place(&[3, 2, 1], true, true);
    let c = tree.place(&[4, 2, 1], true, true);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(tree.nodes.len(), 5);
    assert_eq!(tree.nodes[a as usize].parent, tree.nodes[c as usize].parent);
    // A cut stack hangs under the marker, not under the first frame.
    let cut = tree.place(&[3, 2, 1], false, true);
    assert_ne!(cut, a);
    assert_eq!(tree.nodes[1 + 4].address, CUT);
    // With no room, what is new goes to the one node for it.
    let nodes = tree.nodes.len();
    let full = tree.place(&[9, 8, 1], true, false);
    assert_eq!(tree.nodes[full as usize].address, FULL);
    assert_eq!(tree.nodes.len(), nodes + 1);
    assert_eq!(tree.place(&[3, 2, 1], true, false), a);
}

/// The hooks' own cost and the roles they give: a thread that submits is
/// followed as submitting once it has done so often enough.
#[test]
fn threads_are_followed_by_what_they_were_seen_doing() {
    STATE.store(ON, Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    let (submits, a) = thread({
        let stop = stop.clone();
        move || {
            for _ in 0..SUBMITS {
                seen(Seen::Submit);
            }
            for _ in 0..LIST_CALLS {
                seen(Seen::Record);
            }
            profile_test_sleeps(&stop);
        }
    });
    let (records, b) = thread({
        let stop = stop.clone();
        move || {
            seen(Seen::Submit);
            for _ in 0..LIST_CALLS {
                seen(Seen::Record);
            }
            profile_test_sleeps(&stop);
        }
    });
    let (named, c) = {
        let stop = stop.clone();
        let (said, hears) = mpsc::channel();
        let running = std::thread::Builder::new()
            .name("victory-wide-7".into())
            .spawn(move || {
                // SAFETY: a plain query.
                said.send(unsafe { GetCurrentThreadId() }).unwrap();
                profile_test_sleeps(&stop);
            })
            .unwrap();
        (hears.recv().unwrap(), running)
    };
    std::thread::sleep(Duration::from_millis(50));
    let mut settings = config(1000);
    settings.busy = 0;
    let mut sampler = Sampler::new(settings);
    sampler.rescan();
    let role_of = |id: u32| sampler.threads.iter().find(|t| t.id == id).map(|t| t.role);
    assert_eq!(role_of(submits), Some(Role::Submit));
    assert_eq!(role_of(records), Some(Role::Record));
    assert_eq!(role_of(named), Some(Role::Wide));
    assert!(sampler.threads.iter().all(|t| t.id != sampler.me));
    // A thread that ends is no longer looked at, and its place is free.
    stop.store(true, Relaxed);
    for running in [a, b, c] {
        running.join().unwrap();
    }
    sampler.rescan();
    sampler.rescan();
    assert!(
        sampler
            .threads
            .iter()
            .filter(|t| [submits, records, named].contains(&t.id))
            .all(|t| !t.alive)
    );
    assert!(
        KNOWN_THREADS
            .iter()
            .all(|k| ![submits, records].contains(&k.thread.load(Relaxed)))
    );
    sampler.tick();
}

/// The sampler's own thread, as the game runs it: it finds a worker by
/// its name and a busy thread by its work, and writes the report and the
/// stacks file when its time is up.
#[test]
fn the_sampler_finds_threads_and_writes_its_files() {
    let stop = Arc::new(AtomicBool::new(false));
    let named = {
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("victory-pipelines-9".into())
            .spawn(move || {
                profile_test_sleeps(&stop);
            })
            .unwrap()
    };
    let (_, busy) = thread({
        let stop = stop.clone();
        move || {
            std::hint::black_box(profile_test_spins(&stop));
        }
    });
    let folder = std::env::temp_dir().join(format!("victory_profile_test_{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let mut settings = config(500);
    settings.out = folder.join("profile.txt");
    (settings.every, settings.length, settings.busy) = (1.2, Some(3.0), 2);
    let out = settings.out.clone();
    std::thread::spawn(move || run(settings)).join().unwrap();
    stop.store(true, Relaxed);
    named.join().unwrap();
    busy.join().unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let mut stacks = out.clone().into_os_string();
    stacks.push(".folded");
    let folded = std::fs::read_to_string(&stacks).unwrap();
    let _ = std::fs::remove_dir_all(&folder);
    println!("{text}");
    assert!(text.contains("=== PIPELINE WORKERS"), "{text}");
    assert!(text.contains("victory-pipelines-9"));
    assert!(text.contains("=== OTHER BUSY THREADS"), "{text}");
    // The sleeping thread is seen sleeping: its wake-ups are on the same
    // clock as the sampler's ticks, and are not what is sampled.
    let pipelines = text.split("=== PIPELINE WORKERS").nth(1).unwrap();
    let waiting: f64 = pipelines
        .split("waiting ")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(' ').next())
        .and_then(|share| share.parse().ok())
        .unwrap_or_else(|| panic!("{pipelines}"));
    assert!(
        waiting >= 93.0,
        "the sleeping thread waits {waiting} % of its time"
    );
    assert!(
        folded
            .lines()
            .any(|l| l.starts_with("busy;") && l.contains("profile_test_spin_inner")),
        "{folded}"
    );
    assert!(
        folded
            .lines()
            .any(|l| l.starts_with("pipelines;") && l.contains("profile_test_sleeps")),
        "{folded}"
    );
    // Each stack line ends in its microseconds.
    assert!(folded.lines().all(|l| {
        l.rsplit(' ')
            .next()
            .is_some_and(|n| n.parse::<u64>().is_ok())
    }));
    assert!(DONE.load(Relaxed));
}

/// Measures what the profiler costs the threads it looks at. Not a test
/// of anything: run it by name, in a release build:
/// `cargo test --profile diag -p victory-gpu --features diagnostics
/// profile::tests::what_looking_costs -- --ignored --nocapture`.
///
/// Eight threads do work that is counted, one to a core and above the
/// priority other programs run at, so that a busy machine does not
/// decide the result; rounds with and without the profiler alternate.
#[test]
#[ignore = "a measurement, not a test"]
fn what_looking_costs() {
    use windows_sys::Win32::System::Threading::{SetThreadAffinityMask, THREAD_PRIORITY_HIGHEST};
    #[repr(align(128))]
    struct Count(AtomicU64);
    const THREADS: usize = 8;
    static COUNTS: [Count; THREADS] = [const { Count(AtomicU64::new(0)) }; THREADS];
    static STOP: AtomicBool = AtomicBool::new(false);
    static WAITS: AtomicBool = AtomicBool::new(false);

    #[inline(never)]
    fn profile_cost_work(depth: u32, count: &AtomicU64, seed: u64) -> u64 {
        if depth > 0 {
            return std::hint::black_box(profile_cost_work(
                depth - 1,
                count,
                seed ^ u64::from(depth),
            )) + 1;
        }
        let mut x = seed | 1;
        while !STOP.load(Relaxed) {
            for _ in 0..4096 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
            }
            count.fetch_add(1, Relaxed);
            if WAITS.load(Relaxed) && x.is_multiple_of(64) {
                // Now and then a wait, as a thread that waits for the card has.
                std::thread::sleep(Duration::from_micros(500));
            }
        }
        x
    }

    let pinned = std::thread::available_parallelism().map_or(1, |n| n.get()) >= 2 * THREADS;
    let mut running = Vec::new();
    let mut ids = Vec::new();
    for (i, count) in COUNTS.iter().enumerate() {
        let (id, thread) = thread(move || {
            // SAFETY: plain calls on our own thread.
            unsafe {
                if pinned {
                    SetThreadAffinityMask(GetCurrentThread(), 1 << (2 * i));
                }
                SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
            }
            std::hint::black_box(profile_cost_work(30, &count.0, i as u64 + 1));
        });
        ids.push(id);
        running.push(thread);
    }
    std::thread::sleep(Duration::from_millis(200));
    // The parts of a look at a running thread, each timed on its own.
    {
        // SAFETY: a thread of this test, suspended and resumed in turn; the
        // context is aligned; the copy is ours.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            let thread = OpenThread(ACCESS, 0, ids[0]);
            let mut context = Box::new(Aligned(std::mem::zeroed()));
            let mut copy = vec![0u8; 256 << 10];
            let mut parts: [Vec<u64>; 5] = Default::default();
            for _ in 0..3000 {
                std::thread::sleep(Duration::from_micros(300));
                context.0.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
                let mut got = 0;
                let t0 = Instant::now();
                SuspendThread(thread);
                let t1 = Instant::now();
                GetThreadContext(thread, &mut context.0);
                let t2 = Instant::now();
                ReadProcessMemory(
                    GetCurrentProcess(),
                    context.0.Rsp as *const c_void,
                    copy.as_mut_ptr().cast(),
                    4096,
                    &mut got,
                );
                let t3 = Instant::now();
                ResumeThread(thread);
                let t4 = Instant::now();
                for (part, time) in
                    parts
                        .iter_mut()
                        .zip([t1 - t0, t2 - t1, t3 - t2, t4 - t3, t4 - t0])
                {
                    part.push(time.as_nanos() as u64);
                }
            }
            CloseHandle(thread);
            SetThreadPriority(GetCurrentThread(), 0);
            let median = |part: &mut Vec<u64>| {
                part.sort_unstable();
                (
                    part[part.len() / 2] as f64 / 1000.0,
                    part[part.len() * 9 / 10] as f64 / 1000.0,
                )
            };
            let [a, b, c, d, e] = parts.each_mut().map(median);
            println!(
                "a look at a running thread, 3000 of them, median (and 9 in 10 under), microseconds: \
                 suspend {:.1} ({:.1}), read its registers {:.1} ({:.1}), copy 4 KB of stack {:.1} ({:.1}), \
                 resume {:.1} ({:.1}), all {:.1} ({:.1})",
                a.0, a.1, b.0, b.1, c.0, c.1, d.0, d.1, e.0, e.1
            );
            // The kernel's copy against a plain one, by size.
            let source = vec![7u8; 256 << 10];
            for size in [4usize << 10, 16 << 10, 64 << 10, 128 << 10, 256 << 10] {
                let (mut kernel, mut plain) = (Vec::new(), Vec::new());
                for _ in 0..400 {
                    let mut got = 0;
                    let t0 = Instant::now();
                    ReadProcessMemory(
                        GetCurrentProcess(),
                        source.as_ptr().cast(),
                        copy.as_mut_ptr().cast(),
                        size,
                        &mut got,
                    );
                    let t1 = Instant::now();
                    std::ptr::copy_nonoverlapping(
                        source.as_ptr(),
                        std::hint::black_box(copy.as_mut_ptr()),
                        size,
                    );
                    let t2 = Instant::now();
                    kernel.push((t1 - t0).as_nanos() as u64);
                    plain.push((t2 - t1).as_nanos() as u64);
                }
                println!(
                    "a copy of {} KB: the kernel's {:.1} us, a plain one {:.1} us (medians of 400)",
                    size >> 10,
                    median(&mut kernel).0,
                    median(&mut plain).0
                );
            }
        }
    }
    let total = || COUNTS.iter().map(|c| c.0.load(Relaxed)).sum::<u64>();
    let round = Duration::from_millis(1000);
    let rounds = 9;
    // The timer the sampler ticks by.
    {
        let mut pace = Pace::new(1000, 0);
        let (began, mut longest, mut last) = (Instant::now(), Duration::ZERO, Instant::now());
        for _ in 0..2000 {
            pace.wait();
            longest = longest.max(last.elapsed());
            last = Instant::now();
        }
        println!(
            "the timer: 2000 ticks of a millisecond took {:.1} ms, the longest {:.2} ms (repeating: {})",
            began.elapsed().as_secs_f64() * 1000.0,
            longest.as_secs_f64() * 1000.0,
            pace.repeats
        );
    }
    for (what, waits, reuse, budget) in [
        (
            "8 threads always running, no limit on what looks may cost (VICTORY_PROFILE_BUDGET=0)",
            false,
            true,
            0.0,
        ),
        (
            "8 threads always running, the usual limit of 3 %",
            false,
            true,
            0.03,
        ),
        (
            "8 threads that run and wait, the usual limit",
            true,
            true,
            0.03,
        ),
        (
            "8 threads that run and wait, every look a suspension (VICTORY_PROFILE_REUSE=0), no limit",
            true,
            false,
            0.0,
        ),
    ] {
        WAITS.store(waits, Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        for hz in [1000u32, 4000] {
            let (mut without, mut with) = (Vec::new(), Vec::new());
            let (mut half, mut most, mut longest, mut looks, mut reused, mut late, mut used) =
                (0.0f64, 0.0f64, 0.0f64, 0, 0, 0, 0.0);
            let (mut ticks, mut every) = (0, 1);
            for _ in 0..rounds {
                // Without the profiler.
                let (from, began) = (total(), Instant::now());
                std::thread::sleep(round);
                without.push((total() - from) as f64 / began.elapsed().as_secs_f64());
                // With it, on a thread of its own as in the game.
                let ids = ids.clone();
                let measured = std::thread::spawn(move || {
                    // SAFETY: a plain call on our own thread.
                    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
                    let mut settings = config(hz);
                    settings.reuse = reuse;
                    settings.budget = budget;
                    let mut sampler = Sampler::new(settings);
                    for &id in &ids {
                        sampler.follow(id, Role::Record, null_mut(), None);
                    }
                    sampler.resync();
                    let mut pace = Pace::new(hz, 250);
                    let (cpu, from, began) = (own_cpu(), total(), Instant::now());
                    while began.elapsed() < round {
                        pace.wait();
                        sampler.tick();
                    }
                    let rate = (total() - from) as f64 / began.elapsed().as_secs_f64();
                    let used = (own_cpu() - cpu).as_secs_f64() / began.elapsed().as_secs_f64();
                    let threads = &sampler.threads;
                    (
                        rate,
                        used,
                        sampler.held_within(0.5),
                        sampler.held_within(0.99),
                        sampler.held_longest as f64 / 1000.0,
                        threads.iter().map(|t| t.looks).sum::<u64>(),
                        threads.iter().map(|t| t.reused).sum::<u64>(),
                        sampler.late,
                        threads.iter().map(|t| t.missed).sum::<u64>(),
                        sampler.ticks,
                        threads.iter().map(|t| t.every).max().unwrap_or(1),
                    )
                })
                .join()
                .unwrap();
                with.push(measured.0);
                used += measured.1 / rounds as f64;
                half = half.max(measured.2);
                most = most.max(measured.3);
                longest = longest.max(measured.4);
                looks += measured.5;
                reused += measured.6;
                late += measured.7;
                assert_eq!(measured.8, 0, "looks that failed");
                ticks += measured.9;
                every = every.max(measured.10);
            }
            let median = |values: &mut Vec<f64>| {
                values.sort_by(f64::total_cmp);
                values[values.len() / 2]
            };
            let spread = |values: &[f64]| {
                100.0 * (values[values.len() - 1] - values[0]) / values[values.len() / 2]
            };
            let (off, on) = (median(&mut without), median(&mut with));
            println!(
                "{what}, {hz} looks a second: work done {:.2} % less with the profiler (medians of {rounds} \
                 rounds of 1 s: {off:.0} against {on:.0} a second; the rounds without it differ by {:.1} %, \
                 with it by {:.1} %); a suspension under {half:.0} us half the times, under {most:.0} us 99 \
                 in 100, {longest:.0} us at longest; {ticks} ticks ({late} late), {looks} looks, {reused} \
                 without a suspension, a thread looked at every {every} ticks at most; the sampler's \
                 thread used {:.1} % of a processor",
                100.0 * (off - on) / off,
                spread(&without),
                spread(&with),
                100.0 * used,
            );
        }
    }
    STOP.store(true, Relaxed);
    for thread in running {
        thread.join().unwrap();
    }
}
