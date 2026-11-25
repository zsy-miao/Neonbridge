// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The kernel32 functions that start a child process suspended with pipes for its standard input and output.</summary>
internal static unsafe class ProcessLaunch
{
    /// <summary>CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, CREATE_NO_WINDOW.</summary>
    public const uint CreateSuspended = 0x4;
    public const uint UnicodeEnvironment = 0x400;
    public const uint ExtendedStartupInfo = 0x80000;
    public const uint NoWindow = 0x08000000;

    /// <summary>STARTF_USESTDHANDLES.</summary>
    public const uint UseStandardHandles = 0x100;

    /// <summary>PROC_THREAD_ATTRIBUTE_HANDLE_LIST: the only handles the child inherits.</summary>
    public const nint HandleListAttribute = 0x20002;

    /// <summary>HANDLE_FLAG_INHERIT.</summary>
    public const uint InheritFlag = 0x1;

    private const string Library = "kernel32";

    [DllImport(Library, SetLastError = true)]
    public static extern int CreatePipe(nint* read, nint* write, SecurityAttributes* attributes, uint size);

    [DllImport(Library, SetLastError = true)]
    public static extern int SetHandleInformation(nint handle, uint mask, uint flags);

    [DllImport(Library, SetLastError = true)]
    public static extern int InitializeProcThreadAttributeList(void* list, int count, uint flags, nuint* size);

    [DllImport(Library, SetLastError = true)]
    public static extern int UpdateProcThreadAttribute(void* list, uint flags, nint attribute, void* value, nuint size, void* previous, nuint* returned);

    [DllImport(Library)]
    public static extern void DeleteProcThreadAttributeList(void* list);

    [DllImport(Library, SetLastError = true)]
    public static extern int CreateProcessW(
        char* application, char* commandLine, void* processAttributes, void* threadAttributes, int inheritHandles, uint flags,
        void* environment, char* directory, StartupInfoEx* startup, ProcessInformation* information);

    /// <summary>SECURITY_ATTRIBUTES.</summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct SecurityAttributes
    {
        public int Length;
        public nint Descriptor;
        public int Inherit;
    }

    /// <summary>STARTUPINFOEXW (STARTUPINFOW then the attribute list).</summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct StartupInfoEx
    {
        public int Size;
        public nint Reserved;
        public nint Desktop;
        public nint Title;
        public int X;
        public int Y;
        public int Width;
        public int Height;
        public int CharacterWidth;
        public int CharacterHeight;
        public int FillAttribute;
        public uint Flags;
        public short ShowWindow;
        public short ReservedSize;
        public nint Reserved2;
        public nint StandardInput;
        public nint StandardOutput;
        public nint StandardError;
        public void* AttributeList;
    }

    /// <summary>PROCESS_INFORMATION.</summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct ProcessInformation
    {
        public nint Process;
        public nint Thread;
        public int ProcessId;
        public int ThreadId;
    }
}
