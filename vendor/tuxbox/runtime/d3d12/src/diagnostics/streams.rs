//! TUXBOX_STREAM_WATCH=1: the title's shared command stream table (the
//! ~98-100 s overflow assert at game+0x4dcbc9d). The table pointer lives at
//! game+0x9ae9950 (*0x149ae9950); each slot points at an object whose current
//! stream index is the u32 at +0xc8. A consumer should switch that index every
//! frame (around game+0x4dc3df2/+0x4dc3e11); this logs each slot's index
//! once a second when any changed, and every 10 s regardless, so a stuck
//! index shows. TUXBOX_STREAM_WATCH=rva:offset overrides the two numbers.
//! Pair with TUXBOX_BREAK_RVAS=0x4dc3df2,0x4dc3e11 to see who reaches the switch.
use std::time::{Duration, Instant};

const SLOTS: usize = 16;

pub(crate) fn start(value: &str) {
    let hex = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    let (rva, offset) = match value.split_once(':') {
        Some((a, b)) => (hex(a).unwrap_or(0x9ae_9950), hex(b).unwrap_or(0xc8)),
        None => (0x9ae_9950, 0xc8),
    };
    let Ok(module) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else { return };
    let base = module.0 as usize;
    let _ = std::thread::Builder::new().name("TuxBox stream watch".into()).spawn(move || {
        use super::crash_detail::read;
        let mut last: Vec<Option<u32>> = Vec::new();
        let mut logged = Instant::now() - Duration::from_secs(10);
        loop {
            let table = read::<usize>(base + rva).unwrap_or(0);
            let mut now = Vec::new();
            if table != 0 {
                for slot in 0..SLOTS {
                    let object = read::<usize>(table + 8 * slot).unwrap_or(0);
                    if object == 0 {
                        break;
                    }
                    now.push(read::<u32>(object + offset));
                }
            }
            if now != last || logged.elapsed() >= Duration::from_secs(10) {
                let text = now.iter().enumerate().map(|(i, v)| match v {
                    Some(v) => format!("{i}:{v}"),
                    None => format!("{i}:?"),
                }).collect::<Vec<_>>().join(" ");
                super::log_line(format_args!("stream watch: table game+{rva:#x} = {table:#x}; index +{offset:#x} per slot [{text}]{}",
                    if now == last { " (unchanged)" } else { "" }));
                last = now;
                logged = Instant::now();
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}
