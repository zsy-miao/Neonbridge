# Independent shader GPU checks

These checks extract the listed programs from Frosty’s installed shader archives and compile each with the supplied TuxBox compiler, DXC and DXV. A desktop D3D12 guest draws or dispatches into offscreen resources in a temporary Proton prefix. Each readback must match the expected data for the full resource. The captured results cover the translated shaders and their desktop pipelines. [Measured GPU results](../../../evidence/matrix-awakens/independent-shader-gpu.json)

From the TuxBox checkout, set `STAGE` to a Frosty work directory prepared with the TuxBox Proton fork, `COMPILER` to the Linux `xshader` executable to test, and `WORK` to an output directory, preferably on the same filesystem as the staged prefix. The following commands exercise both shader stages on the tested NVIDIA adapter.

```sh
export VKD3D_CONFIG=cbv_16_byte_alignment VKD3D_DISABLE_EXTENSIONS=VK_EXT_descriptor_buffer
export VKD3D_FILTER_DEVICE_NAME=NVIDIA DXVK_FILTER_DEVICE_NAME=NVIDIA
for kind in pixel compute; do
    python tools/check_independent_shaders.py \
        --stage "$STAGE" --compiler "$COMPILER" --adapter 10de:2705 \
        --cases "tools/reference/independent-shaders/$kind.json" \
        --work "$WORK/$kind" --output "$WORK/$kind.json"
done
```

For the tested Radeon, use `AMD` for both filters, `1002:13c0` for the adapter ID, and point `VK_DRIVER_FILES` at the installed RADV ICD manifest if needed. Select a separate work directory for each adapter and compiler.
