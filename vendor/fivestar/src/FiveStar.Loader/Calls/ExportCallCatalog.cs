// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Calls;

/// <summary>
/// The exports that were disassembled and are safe to call without GPU objects. "--call" refuses anything else: the
/// binaries are untrusted and never called blind.
/// </summary>
internal static class ExportCallCatalog
{
    public static IReadOnlyList<ExportCallRecipe> Recipes { get; } =
    [
        new(BinkRecipes.Module, "BinkUtilCPUs", "BinkUtilCPUs", "mov eax,7; ret", BinkRecipes.CpuCount, true),
        new(BinkRecipes.Module, "BinkGetError", "BinkGetError", "lea rax,[static error buffer]; ret", BinkRecipes.GetError, true),
        new(BinkRecipes.Module, "RADTimerRead", "RADTimerRead", "once-init (QueryPerformanceFrequency) then QPC delta in ms", BinkRecipes.TimerRead, true),
        new(BinkRecipes.Module, "BinkUtilMalloc", "BinkUtilMalloc", "BinkUtilMalloc(size) -> XMemAlloc(size+0x40, 0x05E50000); BinkUtilFree -> XMemFree", BinkRecipes.MallocFree, true),
        new(BvhRecipes.Module, BvhRecipes.Set64BitMortonCodes, "Set64BitMortonCodes", "member fn, ignores this: mov [rip+global],dl; ret", BvhRecipes.ToggleMortonCodes, true),
        new(BvhRecipes.Module, BvhRecipes.PrebuildInfo, "GetRaytracingAccelerationStructurePrebuildInfo", "member fn, object passed nowhere: copies the inputs, CPU size arithmetic, result rounded to 1 KiB", BvhRecipes.ConsoleSizes, true),
        new(BvhRecipes.Module, BvhRecipes.ValidateRehydrate, "ValidateRehydrateAccelerationStructureBuildDesc", "CPU-only validation: XMemVirtualQuery on dest/source, CRC32 of NumDescs vs header+8 -> S_OK / E_INVALIDARG", BvhRecipes.ValidateRehydrateDesc, true),
        new(HttpClientRecipes.Module, "HCGetLibVersion", "HCGetLibVersion", "HCGetLibVersion(const char** out): *out = static string, return S_OK", HttpClientRecipes.LibVersion, true),
        new(HttpClientRecipes.Module, "HCInitialize", "HCInitialize-oom-terminates", "HCMemSetFunctions(fail, nop); HCInitialize(NULL): bad_alloc escapes a noexcept frame -> std::terminate (process exits 0xC0000409)", HttpClientRecipes.InitializeOutOfMemory, false),
        new(HttpClientRecipes.Module, "HCHttpCallCreate", "HCHttpCallCreate-oom", "HCMemSetFunctions(toggle); HCInitialize; [allocator fails] HCHttpCallCreate -> expect E_OUTOFMEMORY via catch; HCCleanup", HttpClientRecipes.CallCreateOutOfMemory, true),
    ];
}
