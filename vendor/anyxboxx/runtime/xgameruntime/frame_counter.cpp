// Compteur d'images du jeu : objectif 60 images par seconde constantes.
//
// Activé seulement si ANYXBOXX_FPS=1 (titre de la fenêtre) ou ANYXBOXX_FPS_CSV=<fichier>
// (une ligne par seconde) : le mode natif redirige alors CreateDXGIFactory* du jeu ici.
// La fabrique rendue garde ses méthodes, sauf la création de swap chain, qui pose un
// relais sur Present et Present1 : chaque image est chronométrée, puis présentée.
#include <windows.h>

#include <dxgi1_3.h>

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <mutex>
#include <vector>

#include "trace.h"

namespace {

// Numéros de méthode dans les tables COM de DXGI (ordre public des interfaces).
constexpr int kFactoryCreateSwapChain = 10;        // IDXGIFactory::CreateSwapChain
constexpr int kFactoryCreateSwapChainForHwnd = 15; // IDXGIFactory2::CreateSwapChainForHwnd
constexpr int kSwapChainPresent = 8;               // IDXGISwapChain::Present
constexpr int kSwapChainPresent1 = 22;             // IDXGISwapChain1::Present1
constexpr double kReportSeconds = 1.0;
constexpr double kSlowestShare = 0.01;             // « 1 % bas »

using CreateSwapChainFn = HRESULT(STDMETHODCALLTYPE*)(IDXGIFactory*, IUnknown*, DXGI_SWAP_CHAIN_DESC*, IDXGISwapChain**);
using CreateForHwndFn = HRESULT(STDMETHODCALLTYPE*)(IDXGIFactory2*, IUnknown*, HWND, const DXGI_SWAP_CHAIN_DESC1*,
                                                     const DXGI_SWAP_CHAIN_FULLSCREEN_DESC*, IDXGIOutput*,
                                                     IDXGISwapChain1**);
using PresentFn = HRESULT(STDMETHODCALLTYPE*)(IDXGISwapChain*, UINT, UINT);
using Present1Fn = HRESULT(STDMETHODCALLTYPE*)(IDXGISwapChain1*, UINT, UINT, const DXGI_PRESENT_PARAMETERS*);

std::atomic<CreateSwapChainFn> g_create_swap_chain{nullptr};
std::atomic<CreateForHwndFn> g_create_for_hwnd{nullptr};
std::atomic<PresentFn> g_present{nullptr};
std::atomic<Present1Fn> g_present1{nullptr};

// Remplace la méthode n° `slot` de la table de `object` ; renvoie l'ancienne (ou nullptr
// si la table porte déjà le relais : DXGI partage une table entre tous ses objets).
void* PatchSlot(void* object, int slot, void* replacement) {
  void** table = *static_cast<void***>(object);
  if (table[slot] == replacement) return nullptr;
  DWORD old = 0;
  if (!VirtualProtect(&table[slot], sizeof(void*), PAGE_READWRITE, &old)) return nullptr;
  void* original = table[slot];
  table[slot] = replacement;
  VirtualProtect(&table[slot], sizeof(void*), old, &old);
  FlushInstructionCache(GetCurrentProcess(), &table[slot], sizeof(void*));
  return original;
}

template <typename Fn>
void Hook(void* object, int slot, Fn replacement, std::atomic<Fn>& original) {
  void* previous = PatchSlot(object, slot, reinterpret_cast<void*>(replacement));
  if (previous != nullptr) original.store(reinterpret_cast<Fn>(previous));
}

// --- Mesure ------------------------------------------------------------------------------
class FrameCounter {
 public:
  void Frame(IDXGISwapChain* swap_chain) {
    LARGE_INTEGER now{};
    QueryPerformanceCounter(&now);
    std::lock_guard<std::mutex> lock(mutex_);
    if (frequency_ == 0) {
      LARGE_INTEGER frequency{};
      QueryPerformanceFrequency(&frequency);
      frequency_ = static_cast<double>(frequency.QuadPart);
      start_ = window_start_ = last_ = now.QuadPart;
      return;
    }
    frame_ms_.push_back(1000.0 * static_cast<double>(now.QuadPart - last_) / frequency_);
    last_ = now.QuadPart;
    const double elapsed = static_cast<double>(now.QuadPart - window_start_) / frequency_;
    if (elapsed < kReportSeconds) return;
    Report(swap_chain, elapsed, static_cast<double>(now.QuadPart - start_) / frequency_);
    frame_ms_.clear();
    window_start_ = now.QuadPart;
  }

 private:
  void Report(IDXGISwapChain* swap_chain, double elapsed, double since_start) {
    std::vector<double> sorted = frame_ms_;
    std::sort(sorted.begin(), sorted.end());
    const size_t frames = sorted.size();
    double total = 0;
    for (double ms : sorted) total += ms;
    const double fps = static_cast<double>(frames) / elapsed;
    const double average = total / static_cast<double>(frames);
    const size_t slow_index = std::min(frames - 1, static_cast<size_t>(static_cast<double>(frames) * (1.0 - kSlowestShare)));
    const double slow_ms = sorted[slow_index];
    ANYXBOXX_TRACE("[fps] %.1f i/s · %.2f ms (1 %% bas : %.1f i/s)", fps, average, 1000.0 / slow_ms);
    WriteCsv(since_start, frames, fps, average, slow_ms);
    ShowInTitle(swap_chain, fps, 1000.0 / slow_ms);
  }

  void WriteCsv(double since_start, size_t frames, double fps, double average, double slow_ms) {
    wchar_t path[MAX_PATH] = {};
    if (GetEnvironmentVariableW(L"ANYXBOXX_FPS_CSV", path, MAX_PATH) == 0) return;
    FILE* file = nullptr;
    if (_wfopen_s(&file, path, L"a") != 0 || file == nullptr) return;
    if (!csv_started_) std::fputs("secondes,images,fps,ms_moyen,ms_p99\n", file);
    csv_started_ = true;
    std::fprintf(file, "%.3f,%zu,%.2f,%.3f,%.3f\n", since_start, frames, fps, average, slow_ms);
    std::fclose(file);
  }

  void ShowInTitle(IDXGISwapChain* swap_chain, double fps, double low_fps) {
    char flag[4] = {};
    if (GetEnvironmentVariableA("ANYXBOXX_FPS", flag, sizeof flag) != 1 || flag[0] != '1') return;
    DXGI_SWAP_CHAIN_DESC desc{};
    if (FAILED(swap_chain->GetDesc(&desc)) || desc.OutputWindow == nullptr) return;
    if (title_.empty()) {
      wchar_t text[256] = {};
      GetWindowTextW(desc.OutputWindow, text, 256);
      title_ = text;
    }
    wchar_t title[384];
    swprintf_s(title, L"%s | AnyXboxX %.0f i/s (1 %% bas %.0f)", title_.c_str(), fps, low_fps);
    SetWindowTextW(desc.OutputWindow, title);
  }

  std::mutex mutex_;
  double frequency_ = 0;
  LONGLONG start_ = 0;
  LONGLONG window_start_ = 0;
  LONGLONG last_ = 0;
  std::vector<double> frame_ms_;
  bool csv_started_ = false;
  std::wstring title_;
};

FrameCounter& Counter() {
  static FrameCounter* counter = new FrameCounter();  // vit jusqu'à la fin du processus
  return *counter;
}

// --- Relais ------------------------------------------------------------------------------
HRESULT STDMETHODCALLTYPE CountedPresent(IDXGISwapChain* self, UINT sync, UINT flags) {
  Counter().Frame(self);
  return g_present.load()(self, sync, flags);
}

HRESULT STDMETHODCALLTYPE CountedPresent1(IDXGISwapChain1* self, UINT sync, UINT flags,
                                          const DXGI_PRESENT_PARAMETERS* parameters) {
  Counter().Frame(self);
  return g_present1.load()(self, sync, flags, parameters);
}

void HookSwapChain(IDXGISwapChain* swap_chain) {
  Hook<PresentFn>(swap_chain, kSwapChainPresent, &CountedPresent, g_present);
  IDXGISwapChain1* swap_chain1 = nullptr;
  if (SUCCEEDED(swap_chain->QueryInterface(IID_PPV_ARGS(&swap_chain1)))) {
    Hook<Present1Fn>(swap_chain1, kSwapChainPresent1, &CountedPresent1, g_present1);
    swap_chain1->Release();
  }
  ANYXBOXX_TRACE("[fps] compteur posé sur la swap chain %p", static_cast<void*>(swap_chain));
}

HRESULT STDMETHODCALLTYPE HookedCreateSwapChain(IDXGIFactory* self, IUnknown* device, DXGI_SWAP_CHAIN_DESC* desc,
                                                IDXGISwapChain** swap_chain) {
  const HRESULT result = g_create_swap_chain.load()(self, device, desc, swap_chain);
  if (SUCCEEDED(result) && swap_chain != nullptr && *swap_chain != nullptr) HookSwapChain(*swap_chain);
  return result;
}

HRESULT STDMETHODCALLTYPE HookedCreateForHwnd(IDXGIFactory2* self, IUnknown* device, HWND window,
                                              const DXGI_SWAP_CHAIN_DESC1* desc,
                                              const DXGI_SWAP_CHAIN_FULLSCREEN_DESC* fullscreen, IDXGIOutput* output,
                                              IDXGISwapChain1** swap_chain) {
  const HRESULT result = g_create_for_hwnd.load()(self, device, window, desc, fullscreen, output, swap_chain);
  if (SUCCEEDED(result) && swap_chain != nullptr && *swap_chain != nullptr) HookSwapChain(*swap_chain);
  return result;
}

void HookFactory(void* factory) {
  Hook<CreateSwapChainFn>(factory, kFactoryCreateSwapChain, &HookedCreateSwapChain, g_create_swap_chain);
  IDXGIFactory2* factory2 = nullptr;
  if (SUCCEEDED(static_cast<IUnknown*>(factory)->QueryInterface(IID_PPV_ARGS(&factory2)))) {
    Hook<CreateForHwndFn>(factory2, kFactoryCreateSwapChainForHwnd, &HookedCreateForHwnd, g_create_for_hwnd);
    factory2->Release();
  }
}

template <typename Create>
HRESULT CreateAndHook(Create create, REFIID iid, void** factory) {
  const HRESULT result = create(iid, factory);
  if (SUCCEEDED(result) && factory != nullptr && *factory != nullptr) HookFactory(*factory);
  return result;
}

}  // namespace

extern "C" {

HRESULT WINAPI AnyXboxX_CreateDXGIFactory(REFIID iid, void** factory) {
  return CreateAndHook([](REFIID id, void** out) { return CreateDXGIFactory(id, out); }, iid, factory);
}

HRESULT WINAPI AnyXboxX_CreateDXGIFactory1(REFIID iid, void** factory) {
  return CreateAndHook([](REFIID id, void** out) { return CreateDXGIFactory1(id, out); }, iid, factory);
}

HRESULT WINAPI AnyXboxX_CreateDXGIFactory2(UINT flags, REFIID iid, void** factory) {
  return CreateAndHook([flags](REFIID id, void** out) { return CreateDXGIFactory2(flags, id, out); }, iid, factory);
}

}  // extern "C"
