# Progress and compatibility

The overall bar gives equal weight to verified library behavior and shader translation. The library percentage counts the reviewed behaviors Frosty can perform. The shader percentage counts entries in its surveyed archives that produced compiler output. Each library behavior counts once, including paths that use packaged or host DLLs. Each verified behavior needs an observable result from the game or a focused check. The percentage describes this workload and moves as games expose new requirements. [Progress map](progress.svg) · [Library audit](LIBRARIES.md) · [Shader survey](SHADERS.md)

Project Scarlett shader preparation compiles 20,583 of 25,079 captured requests (82.1%), with 4,496 unsupported and no compiler failures. The pass took 24 minutes with 16 jobs after a translator update, reusing 2,227 compiled shaders. The captured requests come from recorded runs of this build. [Preparation results](../evidence/projectscarlett-shader-preparation.json)

With the packaged Xbox graphics settings on stock Proton, Frosty rendered its main menu and accepted Start Chase. The game played Wake Up and activated the first street corner of the chase intro. The NVIDIA driver reported a GPU memory fault before the shooting tutorial. Earlier runs with reduced settings reached the shooting tutorial and the city. [Original-settings menu](../evidence/matrix-awakens/packaged-settings-menu.webp) · [Wake Up transition](../evidence/matrix-awakens/packaged-settings-room.json) · [Earlier chase and city](MATRIX_AWAKENS.md)

## Games

| Title | Current result | What remains |
| --- | --- | --- |
| **Lanes** | Runs through TuxBox's Xbox graphics, memory and input providers. On an RTX 4070 Ti SUPER under stock Proton, all 66 guest checks (Lanes' replay, self-test, shader cache and benchmark checks among them) and the D3D12 regression controls pass. Every change must pass them before it is merged. | Add shader forms the translator often refuses, and run the checks on more systems. [Lanes](../examples/lanes/README.md) · [Guest check results](../evidence/lanes-checks-rtx4070tis.json) |
| **The Matrix Awakens (Frosty)** | With the original Xbox graphics settings, a Linux run rendered the menu, played Wake Up and loaded the first street corner of the chase intro. Earlier reduced-settings runs reached the highway and city with missing and distorted surfaces. | Verify the shooting tutorial, image quality and frame times with the packaged settings. [Setup and results](MATRIX_AWAKENS.md) · [Recorded original-settings run](../evidence/matrix-awakens/packaged-settings-room.json) |
| **ProjectScarlett (GTA VI 1.0.2.1)** | An offline Linux run got into the story intro: the game reported `Game Running` at frame 32,534 after 18 m 11 s. GPU queues stalled on fence waits there, and the run ended on a game assert after a command list ran out of descriptor memory. The prologue cutscene has not appeared. [Run record](../evidence/projectscarlett-gameplay.json) · [Startup record](../evidence/projectscarlett-startup.json) | Resolve the remaining fence stalls, reclaim descriptor memory, shorten large driver compiles, support the remaining shaders, then check images, input and frame times. [Setup guide](PROJECTSCARLETT.md#what-to-expect) |
| **Fortnite** | One supplied build provided graphics API call-site evidence. | Test initialization and rendering in TuxBox for a specific build. [Device evidence](../evidence/fortnite-device.json) |

No other retail title is currently claimed playable. Record the build, settings, host, provider and observed faults for each run, and distinguish starting, rendering, input and sustained gameplay.

## Current priorities

### ProjectScarlett

1. Resolve the fence waits that still stall the GPU queues at the story intro. Reproduce each cause as a [Lanes self-test](../examples/lanes/README.md#self-test) scenario first.
2. Reclaim shader-visible descriptor slots, which are never released and run out during long runs.
3. Make large translated compute shaders compile quickly enough. Some now take the driver most of an hour and can hold up loading. [Shader coverage](SHADERS.md#projectscarlett-refusals)
4. Support the remaining refused shader forms, ranked with the [pipeline failure record](PROJECTSCARLETT.md#finding-why-pipelines-fail).

### The Matrix Awakens

1. Finish the five remaining non-ray-tracing pixel shaders. Two light hair strands and three cull distance-field objects. Check their output with the packaged bindings on both GPUs. [Shader survey](SHADERS.md#measured-on-the-matrix-awakens)
2. Identify the GPU command that faults while the first chase street corner loads, then verify the shooting tutorial with the packaged settings. [Retail result](../evidence/matrix-awakens/packaged-settings-room.json)
3. Check the menu, characters, hair, vehicles and city in the game. Test input and audio, then measure cold and warm frame times.
4. Repeat title and provider checks on additional hardware and games as their executables become available.

The default compiler still contains a TomBox-derived HLSL emitter. TuxBox's independent emitter is growing, but it does not yet replace the legacy path for all supported shaders. [Source attribution](REFERENCES.md#shader-compiler-attribution)
