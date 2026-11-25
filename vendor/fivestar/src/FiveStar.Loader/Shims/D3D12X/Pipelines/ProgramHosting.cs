// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;
using FiveStar.Shaders.Compilation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// What <see cref="ProgramProducer.MakeInHost"/> needs: the compile host, the backend compiler that counts the host's
/// verdicts and compiles with dxc what the backend did not take, and the adapter whose form is picked.
/// </summary>
internal sealed record ProgramHosting(CompileHost Host, DxilBackendCompiler Backend, AdapterInfo Adapter);
