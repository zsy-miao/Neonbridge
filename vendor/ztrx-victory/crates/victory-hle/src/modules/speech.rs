//! Accessibility: speech synthesis for the title's narration. No voice is
//! installed here, and every stream is valid WAVE audio of no length, so
//! narration is silent.
// shortcut: silent speech. The host's engine (SAPI through COM, or WinRT
// speech synthesis) needs COM bindings this crate has only in diagnostics
// builds, plus voice enumeration and SSML the title sends (slot 0xa8); the
// title then plays the empty stream and goes on.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{c_char, c_void};

use super::xgameruntime::{
    Guid, HRESULT, Interface, Methods, Object, S_OK, add_ref, missing, put, text,
};

/// Closed captions, high contrast, speech to text and synthesis.
pub const PROVIDER: Guid = Guid::new(
    0x3e24_1977,
    0x6237,
    0x41e9,
    [0x85, 0x59, 0x10, 0x2c, 0x2d, 0x99, 0x83, 0xf1],
);
/// Unchanged since 2020.
pub const IID: Guid = Guid::new(
    0xd722_b373,
    0x8c4d,
    0x4692,
    [0x9e, 0x51, 0xd6, 0xad, 0x9b, 0x37, 0xaa, 0x7d],
);

pub static OBJECT: Object = Object::new("accessibility", &VTABLE);
pub const INTERFACES: &[Interface] = &[(PROVIDER, IID, &OBJECT)];

/// A WAVE file of 16 kHz mono 16-bit PCM with no samples: RIFF size 36,
/// a 16-byte format chunk (PCM, 1 channel, 16,000 Hz, 32,000 bytes/s,
/// block 2, 16 bits), an empty data chunk.
const SILENCE: [u8; 44] =
    *b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\0\0\0\0";

/// Synthesizers and streams hold nothing, so their handles are fixed
/// addresses and closing them is free.
static SYNTHESIZER: u8 = 0;
static STREAM: u8 = 0;

type This = *const Object;

/// No voice is installed, so the callback never runs.
extern "system" fn enumerate_voices(
    _: This,
    _context: *mut c_void,
    _callback: *const c_void,
) -> HRESULT {
    S_OK
}
extern "system" fn create(_: This, out: *mut *const u8) -> HRESULT {
    put(out, &raw const SYNTHESIZER)
}
extern "system" fn close(_: This, _: *const u8) -> HRESULT {
    S_OK
}
extern "system" fn set_default_voice(_: This, _: *const u8) -> HRESULT {
    S_OK
}
extern "system" fn set_custom_voice(_: This, _: *const u8, _voice: *const c_char) -> HRESULT {
    S_OK
}
extern "system" fn create_stream(
    _: This,
    _: *const u8,
    words: *const c_char,
    out: *mut *const u8,
) -> HRESULT {
    tracing::debug!(text = text(words).unwrap_or_default(), "speech (silent)");
    put(out, &raw const STREAM)
}
extern "system" fn stream_size(_: This, _: *const u8, out: *mut usize) -> HRESULT {
    put(out, SILENCE.len())
}
extern "system" fn stream_data(
    _: This,
    _: *const u8,
    size: usize,
    buffer: *mut u8,
    used: *mut usize,
) -> HRESULT {
    if buffer.is_null() || size < SILENCE.len() {
        return super::xasync::E_NOT_SUFFICIENT_BUFFER;
    }
    // SAFETY: the buffer holds `size` bytes, checked above.
    unsafe { buffer.copy_from_nonoverlapping(SILENCE.as_ptr(), SILENCE.len()) };
    if !used.is_null() {
        // SAFETY: checked non-null.
        unsafe { used.write(SILENCE.len()) };
    }
    S_OK
}

static VTABLE: Methods<22> = Methods([
    missing::<0> as *const (),
    add_ref as *const (),
    add_ref as *const (),
    missing::<3> as *const (),      // 0x18 closed captions
    missing::<4> as *const (),      // 0x20
    missing::<5> as *const (),      // 0x28 high contrast
    missing::<6> as *const (),      // 0x30 speech to text
    missing::<7> as *const (),      // 0x38
    enumerate_voices as *const (),  // 0x40
    create as *const (),            // 0x48
    close as *const (),             // 0x50
    set_default_voice as *const (), // 0x58
    set_custom_voice as *const (),  // 0x60
    create_stream as *const (),     // 0x68 from text
    close as *const (),             // 0x70 a stream
    stream_size as *const (),       // 0x78
    stream_data as *const (),       // 0x80
    missing::<17> as *const (),     // 0x88 speech to text
    missing::<18> as *const (),     // 0x90
    missing::<19> as *const (),     // 0x98
    missing::<20> as *const (),     // 0xa0
    create_stream as *const (),     // 0xa8 from SSML
]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::xgameruntime::method;

    #[test]
    fn streams_are_empty_wave_files() {
        let this: This = &OBJECT;
        type Create = extern "system" fn(This, *mut *const u8) -> HRESULT;
        type Stream = extern "system" fn(This, *const u8, *const c_char, *mut *const u8) -> HRESULT;
        type Size = extern "system" fn(This, *const u8, *mut usize) -> HRESULT;
        type Data = extern "system" fn(This, *const u8, usize, *mut u8, *mut usize) -> HRESULT;
        let (mut synth, mut stream, mut size) = (std::ptr::null(), std::ptr::null(), 0);
        assert_eq!(method::<Create>(&OBJECT, 0x48)(this, &mut synth), S_OK);
        let say = method::<Stream>(&OBJECT, 0x68);
        assert_eq!(say(this, synth, c"Hello".as_ptr(), &mut stream), S_OK);
        assert_eq!(method::<Size>(&OBJECT, 0x78)(this, stream, &mut size), S_OK);
        let mut wav = vec![0u8; size];
        let mut used = 0;
        let data = method::<Data>(&OBJECT, 0x80);
        assert_eq!(data(this, stream, size, wav.as_mut_ptr(), &mut used), S_OK);
        assert_eq!(
            (&wav[..4], &wav[8..16], &wav[36..40]),
            (&b"RIFF"[..], &b"WAVEfmt "[..], &b"data"[..])
        );
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(used, 44);
    }
}
