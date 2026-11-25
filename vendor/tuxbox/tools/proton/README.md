# Optional Proton experiment

For normal Linux play, install Valve’s Proton Experimental in Steam. TuxBox’s launcher handles the Xbox constant-buffer offsets with a local Vulkan layer. The instructions below reproduce the earlier experiment that handled those offsets inside vkd3d-proton. [Run Frosty on stock Proton](../../docs/MATRIX_AWAKENS.md#run-with-tuxbox-on-linux) · [Stock GPU results](../../evidence/matrix-awakens/stock-proton-cbv.json)

The fork uses Valve’s `experimental-11.0-20261027b` tag and its pinned vkd3d-proton. The vkd3d-proton patch handles 16-byte Xbox constant-buffer offsets on GPUs with stricter uniform-buffer alignment requirements. The Proton patch installs the feature marker consumed by TuxBox’s launcher. [Valve build instructions](https://github.com/ValveSoftware/Proton/blob/experimental-11.0-20261027b/README.md) · [GPU results and tested revisions](../../evidence/matrix-awakens/proton-cbv.json)

From the TuxBox checkout, with Git and rootless Podman available, run the following in a shell. Choose a build location with enough space for both architectures and the Steam Runtime SDK.

```sh
patch_dir="$PWD/tools/proton"
mkdir -p build/proton-fork
cd build/proton-fork
git clone --branch experimental-11.0-20261027b --recurse-submodules \
    https://github.com/ValveSoftware/Proton.git source
git -C source/vkd3d-proton apply "$patch_dir/vkd3d-proton-cbv.patch"
git -C source apply "$patch_dir/proton-feature.patch"
mkdir build
cd build
../source/configure.sh --container-engine=podman --build-name=tuxbox-proton-experimental-11
make -j6 dist
```

Use the resulting `build/dist` directory with TuxBox’s `--proton` option. The fork was tested in a separate Frosty work directory so the stock and fork builds each have their own prefix. [Frosty setup](../../docs/MATRIX_AWAKENS.md#test-a-proton-fork)

This Valve tag’s Piper dependency may receive HTTP 404 when fetching ONNX Runtime 1.14.1. The tested build used Microsoft’s matching NuGet release. The Linux archive contains the package’s `build/native/include` headers under `onnxruntime-linux-x64-1.14.1/include`, its `runtimes/linux-x64/native/libonnxruntime.so` as `lib/libonnxruntime.so.1.14.1`, and a relative `lib/libonnxruntime.so` symlink to that file. Piper looks for `onnxruntime-linux-x64-1.14.1.tgz` under `obj-piper-x86_64/p/src/piper_phonemize_external-build/download` in the build directory. [Microsoft ONNX Runtime package](https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime/1.14.1)
