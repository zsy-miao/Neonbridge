# XNetworking Rust review

Thanks for the work on XCL. This is a one-off contribution to help with the systems Rust side. I'm not planning to become a recurring reviewer or PR author, but I wanted to give the team something concrete and useful. In my view, XCL is a much more viable long-term approach than Xbox2Windows. Rust is a big part of why. Its types, ownership model and borrow checker let contributors make invariants explicit and have the compiler enforce them. That advantage is worth protecting, especially where Rust meets the Windows and GDK ABIs.

## First change: stop copying struct padding into the async result

In `crates/xcl/src/hle/xgameruntime/networking.rs`, change `default_security_info_bytes` so it no longer copies the raw representation of `XNetworkingSecurityInformation` into a byte vector. `#[repr(C)]` defines the layout, but it does not initialize padding. Zeroing the destination first does not help if the later copy overwrites those bytes.

For this fixed Windows x64 result, start with a zero-initialized buffer sized with `size_of`, then write only the TLS flags at the field offset computed with `offset_of!`. Here is an illustration. It has not been compiled, so treat it as a sketch:

    fn default_security_info_bytes() -> Vec<u8> {
        let mut out = vec![0u8; size_of::<XNetworkingSecurityInformation>()];
        let off = offset_of!(XNetworkingSecurityInformation, enabled_http_security_protocol_flags);
        out[off..off + 4].copy_from_slice(&DEFAULT_TLS_FLAGS.to_le_bytes());
        out
    }

Document why zero bytes represent both a zero thumbprint count and a null thumbprint pointer on the supported target. In the UTF-8 and UTF-16 tests, use storage with the alignment of `XNetworkingSecurityInformation` and assert all three fields: the TLS flags, the zero count and the null pointer. Keep async result consumption and TLS policy unchanged in this first patch. A small patch with a clear regression test is easier to review than a rewrite that mixes ABI, policy and compatibility changes.

## Make the FFI boundary explicit and auditable

- Prefer SDK-backed `windows-sys` types and signatures for WinHTTP, CryptoAPI and IP Helper. `crates/xcl/Cargo.toml` does not currently enable the features for these modules, so enable them first. Then replace handwritten Windows layouts and numeric pointer offsets. The offsets in `extract_chain_certs` should be checked against `wincrypt.h` with `offset_of!` on the SDK types, because they do not look consistent with the documented field order. If a symbol has to be loaded dynamically for older Windows versions, isolate that code, give the function pointer the exact SDK signature, and report a missing DLL or symbol explicitly.
- For GDK types declared in XCL, check size, alignment, field offsets and enum values against the actual GDK contract. A test that repeats the assumptions of our own declaration is not independent ABI evidence.
- Keep `unsafe` in small boundary functions. Give exported unsafe functions a `# Safety` section and put a specific `// SAFETY:` justification on each unsafe operation. State what guarantees the readable or writable extent, alignment, lifetime and aliasing. A null check does not prove a pointer is valid, and `read_unaligned` or `write_unaligned` only removes the alignment requirement. Convert to ordinary Rust types once the boundary contract is established, and keep policy logic safe.
- Use small RAII guards for owned certificate contexts and chains, and tie borrowed chain elements to the lifetime of the guard that owns them. Check buffer size, alignment and output pointers against the API contract before publishing a typed result. Verify how an invalid result call affects retries before changing when `XAsync` consumes a result.

## Keep config, tests and documentation separate

- Config: runtime policy lives in the existing per-title `Config` in `config.rs`, read through `state::config()` and persisted in `xcl.toml`. Validate it at startup. If environment overrides are supported, as `XCL_RAYTRACING` is today, document their precedence and distinguish an unset override from an invalid one. Keep defaults in one place and SDK-defined values as named constants. Do not scatter literals such as a port, a TLS flag set or a fallback connectivity level through entry points.
- Tests: put regression tests in dedicated test files, not at the bottom of the module they exercise. If they need private items, expose a narrow `pub(crate)` surface or use a dedicated `tests` submodule file. Separate pure unit tests from Windows integration tests. Where relevant, cover UTF-8 and UTF-16, short and misaligned buffers, invalid config, malformed pin metadata and native verification failures. Use a controlled local TLS fixture for integration coverage instead of relying only on a synthetic pin matcher. Make sure each test would fail without the fix it claims to cover.
- Documentation: keep explanations, ABI assumptions, supported targets and known gaps in documentation files, not only in code comments. Code comments should state invariants, such as `// SAFETY:` justifications. Docs should say what is implemented, what is a stub and what was actually verified.
- Logs: reuse the existing `xlog_*` macros and file logger. Record failures, compatibility fallbacks and partial implementations with fields that help diagnosis, such as `api`, `reason`, `source` and `hr`. Do not log credentials, full URLs, raw pointers or certificate material. Avoid flooding the log on polling paths, but make it possible to tell that a fallback was used and why.

## Failure must be observable and honest

- Do not report a capability the implementation does not provide. For example, a callback registration that never invokes its callback is not implemented just because it returns a token and logs a warning. A warning is not a substitute for the correct return status. Use the status the API contract specifies, or document and test a deliberate compatibility exception. Likewise, distinguish host connectivity observations from configured fallbacks, and default TLS flags from real per-URL security information.
- Keep certificate verification strict. Do not turn a failed native call into `S_OK` because the captured error code is zero. Do not continue as if hostname verification happened when the hostname could not be obtained. Reject inconsistent pin metadata, such as a nonzero count with a null array, and do not treat an unknown pin type as a match. Check what the GDK means by `Issuer` before changing which certificates can satisfy that pin.
- If local development needs plain HTTP, handle it as a separate, explicit, off-by-default mode. Log when it is used. If the caller requires certificate verification or pinning, plain HTTP must fail rather than report success.

## Working with AI agents

Start with a contract before the agent edits anything: the exact SDK signatures, target assumptions, ownership rules, expected error behavior and acceptance tests. Ask for a small diff with a clear scope. If the documentation does not establish a behavior, ask the agent to name the uncertainty instead of inventing a contract and writing a test that blesses it.

Listen to the compiler. When the borrow checker or type checker rejects a change, ask the agent to explain which invariant it failed to establish. Then fix the type, ownership, lifetime or API design. An AI assistant will often take the shortest path to a green build. Watch for casts, wider `unsafe` blocks, `#[allow]` attributes, deleted assertions and tests that only restate the implementation. The compiler is part of the design process, not an obstacle. A successful build and passing tests on the supported Windows target are required before merging, but neither replaces checking the FFI declarations against the SDK.

## Independent review before every pull request

Before each PR, have the diff reviewed by other agents, two to three rounds, and fix what they find between rounds. These reviewers should be set up for one narrow job each, so the author agent does not review its own work and carry its own bias:

1. A reviewer for types and FFI: SDK signatures, layouts, ownership, cleanup paths and the preconditions of every `unsafe` block.
2. A reviewer for behavior and failure paths: return statuses, output initialization, retries, config validation and log output.
3. A reviewer for evidence and structure: test quality, the separation of config, tests and docs, and whether each claim in the PR description is backed by something that actually ran.

Give each reviewer only the diff, the contract and its own checklist, not the author's reasoning. Ask it to list concrete findings with file and line, not general approval. Reviewer agents can be wrong too, so a human maintainer still decides. Record the findings, what was fixed and what was declined.

Record what actually ran. This review did not include a Windows build or test run. For an implementation, record the compiler target, the checks and tests that ran, and the checks still pending. Do not mark a change as validated until the relevant Windows checks have passed.

Good job, everyone. You're on the right track. I hope this gives the team and its AI assistants a practical way to make the next Rust changes easier to verify, review and maintain.

Krigs
