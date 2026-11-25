# ztrx victory-shader (vendored)

`victory-shader/src` is VItoriaBr's `crates/victory-shader/src` and `build.rs` at commit 2b3c339 (newer than ztrx 2119b0e and VIctory-fxceless a26859f), copied
unmodified (GPL-3.0-or-later, `LICENSE`), credit to the ztrx and VIctory authors
(fxceless, VIctory, and the contributors named in their history). xshader calls
`victory_shader::emit::translate` first under `TB_ZTRX_SHADER` (default on, `=0`
uses only our translator) and falls back to its own HLSL path when ztrx refuses.
