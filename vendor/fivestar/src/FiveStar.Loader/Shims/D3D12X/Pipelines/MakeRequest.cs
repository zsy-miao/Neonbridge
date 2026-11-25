// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// A program for the compile host to make (<see cref="CompileHostProtocol.MakeKind"/>): the extracted program's name,
/// the folder of the FSSM programs, the program cache folder its DXIL goes to, the adapter whose form is picked, and the
/// defines of the loader's modes the source starts with (<see cref="ProgramProducer.Header"/>).
/// </summary>
internal sealed record MakeRequest(string Name, string Programs, string Folder, AdapterInfo Adapter, string Header);
