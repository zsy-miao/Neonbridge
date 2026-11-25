use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn disasm_honors_byte_range_options() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let shader =
        std::env::temp_dir().join(format!("xcl-disasm-{}-{nonce}.bin", std::process::id()));
    std::fs::write(&shader, [0, 0, 0x80, 0xbf, 0, 0, 0x81, 0xbf]).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_xcl"))
            .arg("disasm")
            .arg(&shader)
            .args(args)
            .output()
    };
    let length = run(&["--length", "4"]);
    let offset = run(&["--offset", "0x4", "--length", "0x4"]);
    let malformed = run(&["--offset", "bogus"]);
    let missing = run(&["--length"]);
    std::fs::remove_file(&shader).unwrap();

    let length = length.unwrap();
    assert!(length.status.success());
    let listing = String::from_utf8(length.stdout).unwrap();
    assert!(listing.contains("00000000: s_nop"), "{listing}");
    assert!(!listing.contains("s_endpgm"), "{listing}");

    let offset = offset.unwrap();
    assert!(offset.status.success());
    let listing = String::from_utf8(offset.stdout).unwrap();
    assert!(listing.contains("00000004: s_endpgm"), "{listing}");
    assert!(!listing.contains("s_nop"), "{listing}");

    for (output, flag) in [(malformed, "--offset"), (missing, "--length")] {
        let output = output.unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr).unwrap().contains(flag));
    }
}
