//! `amd_fidelityfx_loader_xbox.dll` and `amd_fidelityfx_upscaler_xbox.dll`,
//! which both export the five FidelityFX calls: with `VICTORY_FFX_DIR` set,
//! served by AMD's PC upscaler on the host GPU (`victory_gpu::ffx`) instead
//! of the console DLLs, whose tables only cover a 1920x1080 output.

use crate::{HleExport, HleModule};

pub(crate) fn module() -> HleModule {
    use victory_gpu::ffx;
    let export = |name, address: usize| HleExport { name, address };
    HleModule {
        name: "ffx",
        dlls: &[
            "amd_fidelityfx_loader_xbox.dll",
            "amd_fidelityfx_upscaler_xbox.dll",
        ],
        exports: vec![
            export(
                "ffxCreateContext",
                ffx::ffxCreateContext as *const () as usize,
            ),
            export(
                "ffxDestroyContext",
                ffx::ffxDestroyContext as *const () as usize,
            ),
            export("ffxConfigure", ffx::ffxConfigure as *const () as usize),
            export("ffxQuery", ffx::ffxQuery as *const () as usize),
            export("ffxDispatch", ffx::ffxDispatch as *const () as usize),
        ],
    }
}
