// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The compile host's answer to a request (<see cref="CompileHostProtocol"/>): its kind, why not, and for a make that
/// dxc must finish, the form's profile, source file name and source.
/// </summary>
internal sealed record HostAnswer(HostAnswerKind Kind, string Text, string Profile, string Source, string Hlsl)
{
    public static HostAnswer Compiled { get; } = Of(BackendVerdict.Compiled, string.Empty);

    /// <summary>An answer of a compile request.</summary>
    public static HostAnswer Of(BackendVerdict verdict, string reason)
    {
        return new HostAnswer((HostAnswerKind)verdict, reason, string.Empty, string.Empty, string.Empty);
    }

    /// <summary>A make's failure that repeats (translator refusal, no form for the adapter); the host kept it as a .fail.</summary>
    public static HostAnswer Refused(string failure)
    {
        return new HostAnswer(HostAnswerKind.Refused, failure, string.Empty, string.Empty, string.Empty);
    }

    /// <summary>A request the host could not serve (an error): the loader does the work itself.</summary>
    public static HostAnswer Failed(string reason)
    {
        return new HostAnswer(HostAnswerKind.Failed, reason, string.Empty, string.Empty, string.Empty);
    }

    /// <summary>The backend's verdict of a compile answer.</summary>
    public BackendVerdict Verdict => (BackendVerdict)Kind;
}
