// État HDR de l'écran (XDisplayTryEnableHdrMode) et connectivité réseau (XNetworking) du PC.
//
// Structures et signatures : XDisplay.h et XNetworking.h du GDK public. Les réponses viennent
// de Windows : DXGI (IDXGIOutput6::GetDesc1) pour l'écran principal, Network List Manager et
// la table de routage pour le réseau. Sur PC, c'est Windows qui allume le HDR : le jeu
// reçoit l'état réel de l'écran, jamais un HDR simulé.
#include <winsock2.h>
#include <windows.h>
#include <dxgi1_6.h>
#include <iphlpapi.h>
#include <netlistmgr.h>

#include <cstdint>

#include "hdr_luminance.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

template <typename T>
struct ComPtr {
  T* raw = nullptr;
  ~ComPtr() {
    if (raw != nullptr) raw->Release();
  }
  T** Out() { return &raw; }
  T* operator->() const { return raw; }
};

// --- XDisplay : HDR de l'écran principal ----------------------------------------------
enum HdrModeResult : uint32_t { kHdrUnknown = 0, kHdrEnabled = 1, kHdrDisabled = 2 };

using anyxboxx::HdrModeInfo;

bool PrimaryOutputDescription(DXGI_OUTPUT_DESC1* description) {
  const HMONITOR primary = MonitorFromPoint(POINT{0, 0}, MONITOR_DEFAULTTOPRIMARY);
  ComPtr<IDXGIFactory1> factory;
  if (FAILED(CreateDXGIFactory1(__uuidof(IDXGIFactory1), reinterpret_cast<void**>(factory.Out())))) return false;
  for (UINT adapter_index = 0;; ++adapter_index) {
    ComPtr<IDXGIAdapter1> adapter;
    if (factory->EnumAdapters1(adapter_index, adapter.Out()) == DXGI_ERROR_NOT_FOUND) return false;
    for (UINT output_index = 0;; ++output_index) {
      ComPtr<IDXGIOutput> output;
      if (adapter->EnumOutputs(output_index, output.Out()) == DXGI_ERROR_NOT_FOUND) break;
      ComPtr<IDXGIOutput6> output6;
      if (FAILED(output->QueryInterface(__uuidof(IDXGIOutput6), reinterpret_cast<void**>(output6.Out())))) continue;
      if (SUCCEEDED(output6->GetDesc1(description)) && description->Monitor == primary) return true;
    }
  }
}

HdrModeResult XDisplayTryEnableHdrMode(uint32_t preference, HdrModeInfo* info) noexcept {
  DXGI_OUTPUT_DESC1 description{};
  if (!PrimaryOutputDescription(&description)) return kHdrUnknown;
  const bool hdr = description.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
  if (hdr && info != nullptr) {
    *info = anyxboxx::NormalizeHdrLuminance(
        HdrModeInfo{description.MinLuminance, description.MaxLuminance, description.MaxFullFrameLuminance});
  }
  ANYXBOXX_TRACE("[runtime] XDisplayTryEnableHdrMode(%u) : écran %s (DXGI : %.4f / %.1f / %.1f nits)", preference,
                 hdr ? "HDR" : "SDR", description.MinLuminance, description.MaxLuminance,
                 description.MaxFullFrameLuminance);
  return hdr ? kHdrEnabled : kHdrDisabled;
}

// --- XNetworking : indice de connectivité ---------------------------------------------
enum ConnectivityLevel : uint32_t { kLevelUnknown = 0, kLevelNone = 1, kLevelLocal = 2, kLevelInternet = 3 };

struct ConnectivityHint {
  uint32_t connectivity_level;
  uint32_t connectivity_cost;        // 0 : inconnu (le coût vient de l'API WinRT, non interrogée)
  uint32_t iana_interface_type;
  bool network_initialized;
  bool approaching_data_limit;
  bool over_data_limit;
  bool roaming;
};

uint32_t ConnectivityLevelFromWindows() {
  const HRESULT init = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
  uint32_t level = kLevelUnknown;
  {
    ComPtr<INetworkListManager> manager;
    NLM_CONNECTIVITY connectivity = NLM_CONNECTIVITY_DISCONNECTED;
    if (SUCCEEDED(CoCreateInstance(CLSID_NetworkListManager, nullptr, CLSCTX_ALL, __uuidof(INetworkListManager),
                                   reinterpret_cast<void**>(manager.Out()))) &&
        SUCCEEDED(manager->GetConnectivity(&connectivity))) {
      if (connectivity & (NLM_CONNECTIVITY_IPV4_INTERNET | NLM_CONNECTIVITY_IPV6_INTERNET)) {
        level = kLevelInternet;
      } else if (connectivity & (NLM_CONNECTIVITY_IPV4_LOCALNETWORK | NLM_CONNECTIVITY_IPV6_LOCALNETWORK |
                                 NLM_CONNECTIVITY_IPV4_SUBNET | NLM_CONNECTIVITY_IPV6_SUBNET)) {
        level = kLevelLocal;
      } else {
        level = kLevelNone;
      }
    }
  }
  if (SUCCEEDED(init)) CoUninitialize();
  return level;
}

// Type IANA (ifType) de l'interface de la route par défaut, 0 s'il n'y en a pas.
uint32_t DefaultRouteInterfaceType() {
  DWORD index = 0;
  if (GetBestInterface(0, &index) != NO_ERROR) return 0;
  MIB_IFROW row{};
  row.dwIndex = index;
  return GetIfEntry(&row) == NO_ERROR ? static_cast<uint32_t>(row.dwType) : 0;
}

HRESULT XNetworkingGetConnectivityHint(ConnectivityHint* hint) noexcept {
  if (hint == nullptr) return E_INVALIDARG;
  *hint = ConnectivityHint{};
  hint->connectivity_level = ConnectivityLevelFromWindows();
  if (hint->connectivity_level == kLevelUnknown) hint->connectivity_level = kLevelNone;
  hint->iana_interface_type = DefaultRouteInterfaceType();
  hint->network_initialized = true;
  return S_OK;
}

// Les changements de connectivité ne sont pas suivis : l'abonnement reçoit un jeton, sans rappel.
HRESULT RegisterHintChanged(void*, void*, void* callback, void* token) noexcept {
  return anyxboxx::AddSubscription(callback, token);
}
bool UnregisterHintChanged(uint64_t token, bool) noexcept { return anyxboxx::RemoveSubscription(token); }

void* const kHdrMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                  // 0-2
    ANYXBOXX_NOT_IMPLEMENTED,                   // 3 (absente de gdk_api_table.inc)
    ANYXBOXX_METHOD(XDisplayTryEnableHdrMode),  // 4
};

// XNetworking : méthodes 3 à 20 (gdk_api_table.inc) ; seules 15 à 17 sont écrites.
void* const kNetworkingMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,  // 0-2
    ANYXBOXX_NOT_IMPLEMENTED,  // 3 XNetworkingQueryPreferredLocalUdpMultiplayerPort
    ANYXBOXX_NOT_IMPLEMENTED,  // 4 XNetworkingQueryPreferredLocalUdpMultiplayerPortAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 5 XNetworkingQueryPreferredLocalUdpMultiplayerPortAsyncResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 6 XNetworkingRegisterPreferredLocalUdpMultiplayerPortChanged
    ANYXBOXX_NOT_IMPLEMENTED,  // 7 XNetworkingUnregisterPreferredLocalUdpMultiplayerPortChanged
    ANYXBOXX_NOT_IMPLEMENTED,  // 8 XNetworkingQuerySecurityInformationForUrlAsync
    ANYXBOXX_NOT_IMPLEMENTED,  // 9 XNetworkingQuerySecurityInformationForUrlAsyncResultSize
    ANYXBOXX_NOT_IMPLEMENTED,  // 10 XNetworkingQuerySecurityInformationForUrlAsyncResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 11 XNetworkingQuerySecurityInformationForUrlUtf16Async
    ANYXBOXX_NOT_IMPLEMENTED,  // 12 XNetworkingQuerySecurityInformationForUrlUtf16AsyncResultSize
    ANYXBOXX_NOT_IMPLEMENTED,  // 13 XNetworkingQuerySecurityInformationForUrlUtf16AsyncResult
    ANYXBOXX_NOT_IMPLEMENTED,  // 14 XNetworkingVerifyServerCertificate
    ANYXBOXX_METHOD(XNetworkingGetConnectivityHint),  // 15
    ANYXBOXX_METHOD(RegisterHintChanged),  // 16 XNetworkingRegisterConnectivityHintChanged
    ANYXBOXX_METHOD(UnregisterHintChanged),  // 17 XNetworkingUnregisterConnectivityHintChanged
    ANYXBOXX_NOT_IMPLEMENTED,  // 18 XNetworkingQueryConfigurationSetting
    ANYXBOXX_NOT_IMPLEMENTED,  // 19 XNetworkingSetConfigurationSetting
    ANYXBOXX_NOT_IMPLEMENTED,  // 20 XNetworkingQueryStatistics
};

const anyxboxx::StaticInterface kHdr{kHdrMethods};
const anyxboxx::StaticInterface kNetworking{kNetworkingMethods};

}  // namespace

namespace anyxboxx {

void* HdrDisplayInterface() { return const_cast<StaticInterface*>(&kHdr); }
void* NetworkingInterface() { return const_cast<StaticInterface*>(&kNetworking); }

}  // namespace anyxboxx

extern "C" {

// Règle de repli des luminances HDR (hdr_luminance.h), exposée pour les tests : l'écran du PC ne
// permet pas de choisir les valeurs que DXGI rend.
void WINAPI AnyXboxX_NormalizeHdrLuminance(anyxboxx::HdrModeInfo* info) {
  if (info != nullptr) *info = anyxboxx::NormalizeHdrLuminance(*info);
}

}  // extern "C"
