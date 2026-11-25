// Adaptateur choisi par son nom et description pour le banc GPU (tools/Run-GpuCheck.ps1).
#include "adapter.h"

#include <dxgi1_6.h>

#include <cstdio>
#include <cwctype>
#include <string>
#include <vector>

using Microsoft::WRL::ComPtr;

namespace {

constexpr D3D_FEATURE_LEVEL kFeatureLevel = D3D_FEATURE_LEVEL_11_0;

std::wstring Variable() {
  const DWORD size = GetEnvironmentVariableW(L"ANYXBOXX_GPU", nullptr, 0);
  if (size == 0) return {};
  std::wstring value(size, L'\0');
  value.resize(GetEnvironmentVariableW(L"ANYXBOXX_GPU", value.data(), size));
  return value;
}

std::wstring Lower(std::wstring text) {
  for (wchar_t& c : text) c = static_cast<wchar_t>(std::towlower(c));
  return text;
}

std::string Utf8(const std::wstring& text) {
  if (text.empty()) return {};
  const int length = static_cast<int>(text.size());
  const int size = WideCharToMultiByte(CP_UTF8, 0, text.data(), length, nullptr, 0, nullptr, nullptr);
  std::string out(static_cast<size_t>(size), '\0');
  WideCharToMultiByte(CP_UTF8, 0, text.data(), length, out.data(), size, nullptr, nullptr);
  return out;
}

std::wstring Name(IDXGIAdapter1* adapter) {
  DXGI_ADAPTER_DESC1 desc = {};
  adapter->GetDesc1(&desc);
  return desc.Description;
}

// Adaptateurs matériels (sans DXGI_ADAPTER_FLAG_SOFTWARE), dans l'ordre de DXGI.
std::vector<ComPtr<IDXGIAdapter1>> HardwareAdapters(IDXGIFactory4* factory) {
  std::vector<ComPtr<IDXGIAdapter1>> adapters;
  ComPtr<IDXGIAdapter1> adapter;
  for (UINT i = 0; factory->EnumAdapters1(i, &adapter) != DXGI_ERROR_NOT_FOUND; ++i) {
    DXGI_ADAPTER_DESC1 desc = {};
    if (SUCCEEDED(adapter->GetDesc1(&desc)) && !(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE)) adapters.push_back(adapter);
  }
  return adapters;
}

ComPtr<IDXGIAdapter1> Match(IDXGIFactory4* factory, const std::wstring& filter) {
  const std::wstring wanted = Lower(filter);
  for (const ComPtr<IDXGIAdapter1>& adapter : HardwareAdapters(factory)) {
    if (Lower(Name(adapter.Get())).find(wanted) != std::wstring::npos) return adapter;
  }
  return nullptr;
}

void ReportNoMatch(IDXGIFactory4* factory, const std::wstring& filter) {
  std::string names;
  for (const ComPtr<IDXGIAdapter1>& adapter : HardwareAdapters(factory)) {
    names += (names.empty() ? "" : ", ") + Utf8(Name(adapter.Get()));
  }
  std::fprintf(stderr, "[gpu] aucun adaptateur matériel ne contient « %s » ; adaptateurs : %s\n", Utf8(filter).c_str(),
               names.empty() ? "aucun" : names.c_str());
}

// L'adaptateur que CreateDevice (gpu_run.cpp) prendrait ; nullptr si le nom demandé ne correspond à aucun.
ComPtr<IDXGIAdapter1> ChosenAdapter(IDXGIFactory4* factory) {
  ComPtr<IDXGIAdapter1> adapter;
  std::wstring filter;
  if (Variable() == L"warp") {
    factory->EnumWarpAdapter(IID_PPV_ARGS(&adapter));
  } else if (NamedAdapterRequested(filter)) {
    adapter = Match(factory, filter);
    if (!adapter) ReportNoMatch(factory, filter);
  } else if (FAILED(factory->EnumAdapters1(0, &adapter)) ||
             FAILED(D3D12CreateDevice(adapter.Get(), kFeatureLevel, __uuidof(ID3D12Device), nullptr))) {
    factory->EnumWarpAdapter(IID_PPV_ARGS(&adapter));  // comme CreateDevice : WARP si l'adaptateur par défaut échoue
  }
  return adapter;
}

void Describe(IDXGIAdapter1* adapter, ID3D12Device* device) {
  DXGI_ADAPTER_DESC1 desc = {};
  adapter->GetDesc1(&desc);
  std::printf("adapter=%s vendor=%04x device=%04x\n", Utf8(desc.Description).c_str(), desc.VendorId, desc.DeviceId);
  LARGE_INTEGER version = {};
  if (SUCCEEDED(adapter->CheckInterfaceSupport(__uuidof(IDXGIDevice), &version))) {
    std::printf("driver=%u.%u.%u.%u\n", static_cast<unsigned>(HIWORD(version.HighPart)),
                static_cast<unsigned>(LOWORD(version.HighPart)), static_cast<unsigned>(HIWORD(version.LowPart)),
                static_cast<unsigned>(LOWORD(version.LowPart)));
  } else {
    std::printf("driver=inconnu\n");
  }
  std::printf("backend=%s\n", (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) ? "warp" : "hardware");
  D3D12_FEATURE_DATA_D3D12_OPTIONS1 lanes = {};
  device->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS1, &lanes, static_cast<UINT>(sizeof lanes));
  std::printf("native_lane_range=%u-%u\n", lanes.WaveLaneCountMin, lanes.WaveLaneCountMax);
  D3D12_FEATURE_DATA_D3D12_OPTIONS5 raytracing = {};
  device->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS5, &raytracing, static_cast<UINT>(sizeof raytracing));
  const int tier = raytracing.RaytracingTier;
  if (tier == D3D12_RAYTRACING_TIER_NOT_SUPPORTED) {
    std::printf("raytracing_tier=aucun\n");
  } else {
    std::printf("raytracing_tier=%d.%d\n", tier / 10, tier % 10);
  }
  // Doubles dans les shaders : absents sur certains GPU intégrés (Intel UHD), qui refusent alors le DXIL.
  D3D12_FEATURE_DATA_D3D12_OPTIONS options = {};
  device->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS, &options, static_cast<UINT>(sizeof options));
  std::printf("double_precision=%s\n", options.DoublePrecisionFloatShaderOps ? "oui" : "non");
}

}  // namespace

bool NamedAdapterRequested(std::wstring& filter) {
  const std::wstring value = Variable();
  if (value.empty() || Lower(value) == L"warp") return false;
  filter = value;
  return true;
}

ComPtr<ID3D12Device> CreateNamedDevice(const std::wstring& filter) {
  ComPtr<IDXGIFactory4> factory;
  if (FAILED(CreateDXGIFactory2(0, IID_PPV_ARGS(&factory)))) return nullptr;
  const ComPtr<IDXGIAdapter1> adapter = Match(factory.Get(), filter);
  if (!adapter) {
    ReportNoMatch(factory.Get(), filter);
    return nullptr;
  }
  ComPtr<ID3D12Device> device;
  const HRESULT result = D3D12CreateDevice(adapter.Get(), kFeatureLevel, IID_PPV_ARGS(&device));
  if (FAILED(result)) {
    std::fprintf(stderr, "[gpu] pas de périphérique D3D12 sur « %s » (0x%08lx)\n", Utf8(Name(adapter.Get())).c_str(),
                 static_cast<unsigned long>(result));
    return nullptr;
  }
  return device;
}

int PrintAdapter() {
  ComPtr<IDXGIFactory4> factory;
  if (FAILED(CreateDXGIFactory2(0, IID_PPV_ARGS(&factory)))) return 1;
  const ComPtr<IDXGIAdapter1> adapter = ChosenAdapter(factory.Get());
  ComPtr<ID3D12Device> device;
  if (!adapter) return 1;
  if (FAILED(D3D12CreateDevice(adapter.Get(), kFeatureLevel, IID_PPV_ARGS(&device)))) {
    std::fprintf(stderr, "[gpu] pas de périphérique D3D12 sur « %s »\n", Utf8(Name(adapter.Get())).c_str());
    return 1;
  }
  Describe(adapter.Get(), device.Get());
  return 0;
}
