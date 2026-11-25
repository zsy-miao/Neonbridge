// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>What a pipeline host needs besides the stream (<see cref="PipelineHostProtocol"/>).</summary>
/// <param name="AdapterLuid">The LUID of the adapter the loader's device is on.</param>
/// <param name="RootSignature">The serialized root signature the stream names.</param>
/// <param name="AllocateBytes">Bytes the host allocates and touches instead of compiling (a test of the memory limit); 0 compiles.</param>
internal sealed record PipelineHostRequest(long AdapterLuid, byte[] RootSignature, long AllocateBytes);
