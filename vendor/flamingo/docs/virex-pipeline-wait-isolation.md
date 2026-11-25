# Isolate pipeline waits during streaming and startup

The fork's pipeline workers publish completion through a global condition
variable. Every finished pipeline wakes threads waiting on unrelated
pipelines. Move the completion notification to each Pending pipeline,
retaining the same publication-before-notification protocol and timeout.

The shared per-frame wait budget was reset on any differing frame number.
A delayed caller with an older frame could erase the newer frame's spent
or reserved budget. Ignore such stale reservations and completions.
The title frame counter is monotonically increasing for a normal run;
wraparound is not handled by this budget (u64 frames).

Keep the configured budget, queue priorities, compiler and draw fallback
behavior unchanged. This is not the intro texture flicker fix (#343).
It targets CPU wakeups and accidental budget resets during pipeline
compilation, including cold-cache startup. Each Pending gains a mutex
and condition variable; no extra heap allocations are introduced here
explicitly, but per-pipeline storage increases.

Two Rust regression tests for budget transitions are added. A JS model
checked 524,288 seeded reservation/completion events in shared mode
against a separate ledger of active reservations and finished waits:
zero disagreements. Native Rust and Windows runtime tests pending.
No measured startup time or FPS improvement is claimed.

    cargo test -p victory-gpu pipeline::make::tests --locked

No new dependencies or game assets. AI-assisted (ChatGPT).
