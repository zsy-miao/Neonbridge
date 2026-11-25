// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Loading;

/// <summary>How the loader maps images and binds their imports.</summary>
/// <param name="SearchDirectories">Directories searched for modules mapped by this loader; the directory of every loaded image is added.</param>
/// <param name="ManualModules">Module names (lower case, with extension) mapped by this loader instead of the OS.</param>
/// <param name="Unresolved">What happens to imports nothing resolves.</param>
/// <param name="CallEntryPoints">Whether DllMain runs.</param>
/// <param name="WorkDirectory">Where generated TLS host DLLs are written.</param>
/// <param name="MainModule">The image that plays the process executable; null picks it automatically.</param>
/// <param name="ProcessId">Distinguishes the TLS host files of concurrent processes.</param>
internal sealed record LoaderOptions(
    IReadOnlyList<string> SearchDirectories,
    IReadOnlySet<string> ManualModules,
    UnresolvedMode Unresolved,
    bool CallEntryPoints,
    string WorkDirectory,
    string? MainModule,
    int ProcessId);
