// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger;

/// <summary>The fivestar-dbg help text.</summary>
internal static class Usage
{
    public const string Text = """
        usage: fivestar-dbg [options] [--] <program> [args...]

        Runs a program under the Win32 debugger and logs module loads, exceptions (code, address, module+offset,
        symbol), registers and symbolised thread stacks; the exit code is the program's.

          --debug-heap               ntdll's debug heap (tail and free checks) and full page heap for this process
          --break-on <code,...>      full record already at first chance (hex codes or names like AccessViolation)
          --symbols <path>           folders and srv*<cache>*<url> entries; default _NT_SYMBOL_PATH
          --dump-on-crash <file>     minidump at the first second-chance exception
          --full-dump                that minidump with all process memory
          --log <file>               copy of the output
          --loader-log <file>        fivestar-loader log naming the images it mapped (default: the program's --log)
          --children                 also debug the processes the program starts
          --timeout <s>              end the program after s seconds
        """;
}
