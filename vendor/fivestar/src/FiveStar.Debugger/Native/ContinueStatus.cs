// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>How ContinueDebugEvent resumes the thread that reported an event (ntstatus.h).</summary>
internal enum ContinueStatus : uint
{
    /// <summary>DBG_CONTINUE: the exception is handled; the thread resumes where it stopped.</summary>
    Continue = 0x00010002,

    /// <summary>DBG_EXCEPTION_NOT_HANDLED: the exception goes on to the program's own handlers.</summary>
    ExceptionNotHandled = 0x80010001,
}
