//! `neonbridge ctl <command>`: talk to the running game through the local
//! named pipe \\.\pipe\tuxbox-ctl that the d3d12 provider serves (no network).
use std::error::Error;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub(crate) const USAGE: &str = "usage: neonbridge ctl <command>   (talks to the running game; local pipe only)
  status                 fps, frame split, GPU latency, hot threads, audio counters
  log [N] [filter]       the last N (default 40) log lines containing the filter
  stack                  every thread's stack now
  where ADDR             what game+0xRVA or 0xVA is now (module+RVA, export, memory kind, bytes)
  press BUTTONS [ms]     A B X Y Menu View Up Down Left Right LB RB LS RS, join with + (default 150 ms)
  hold BUTTONS | release
  screenshot FILE        the next presented frame as .bmp
  get [NAME] | set NAME VALUE | unset NAME   live TUXBOX_* switches
  stop                   close the game";

pub(crate) fn run(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let mut words: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if words.is_empty() || matches!(words[0].as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(());
    }
    // A relative screenshot path is the caller's, not the game's working folder.
    if words[0].eq_ignore_ascii_case("screenshot") && words.len() > 1 {
        words[1] = std::path::absolute(&words[1])?.display().to_string();
    }
    let line = words.join(" ");
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut pipe = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(r"\\.\pipe\tuxbox-ctl") {
            Ok(pipe) => break pipe,
            // 231 = all pipe instances busy (another request is being served).
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) if e.raw_os_error() == Some(2) => {
                return Err("no running game is listening on \\\\.\\pipe\\tuxbox-ctl (start it with neonbridge; TUXBOX_CTL=0 turns the channel off)".into());
            }
            Err(e) => return Err(format!("\\\\.\\pipe\\tuxbox-ctl: {e}").into()),
        }
    };
    pipe.write_all(format!("{line}\n").as_bytes())?;
    // The server writes, flushes and disconnects: ERROR_BROKEN_PIPE (109) or
    // ERROR_PIPE_NOT_CONNECTED (233) after the reply is the normal end, and the
    // bytes read before it are the reply.
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.raw_os_error(), Some(109 | 233)) => break,
            Err(e) => return Err(e.into()),
        }
    }
    print!("{}", String::from_utf8_lossy(&reply));
    Ok(())
}
