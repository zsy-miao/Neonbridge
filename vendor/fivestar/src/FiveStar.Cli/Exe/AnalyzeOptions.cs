// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Exe;

/// <summary>The parsed arguments of <c>fivestar exe analyze</c>.</summary>
/// <param name="ExePath">The executable to analyze.</param>
/// <param name="DumpFolder">The dump folder, or null; its <c>*.rpf</c> names seed the archive-reference search.</param>
/// <param name="GdkLib">The PC <c>xgameruntime.lib</c>, or null; it names the GDK thunks.</param>
/// <param name="OutDir">Where the report is written.</param>
/// <param name="LoaderPath">The <c>fivestar-loader</c> executable, or null to locate it automatically.</param>
public sealed record AnalyzeOptions(string ExePath, string? DumpFolder, string? GdkLib, string OutDir, string? LoaderPath);
