// D3D12ConfigureVirtualMemoryX de d3d12_xs.dll : configuration de la mémoire graphique, appelée
// par le titre avant la création du périphérique (sans cet export, le titre s'arrête en
// « UNIMPLEMENTED d3d12_xs.dll!D3D12ConfigureVirtualMemoryX »).
//
// Disposition de la structure, d'après TomBox (crates/compat/d3d12_xs/src/vmem_x.rs, struct
// VirtualMemoryConfiguration) : flags, page_table_pages, frame_buffer_preserve_bytes, reserved
// (quatre mots de 32 bits), output0 et output1 (deux mots de 64 bits).
//
// Sur l'exécutable réel, le titre ne remplit que le mot à +4 (page_table_pages) et ne teste que
// le résultat ; les pointeurs passés dans le cadre de pile de l'appelant ne garantissent pas
// 32 octets. Mesure de Leonida (runtime/shims/d3d12_xs.cpp, lignes 221-223, GitHub Leonida
// #6750) : leur shim n'écrit rien et rend S_OK. Ici aussi : on ne lit que flags et
// page_table_pages (tracés), on n'écrit aucune sortie. Un pointeur nul est refusé.
#include <windows.h>

#include <cstdint>

#include "trace.h"

namespace {

struct VirtualMemoryConfigurationHead {
  std::uint32_t flags;
  std::uint32_t page_table_pages;
};

}  // namespace

extern "C" {

HRESULT WINAPI D3D12ConfigureVirtualMemoryX(VirtualMemoryConfigurationHead* description) {
  if (description == nullptr) return E_INVALIDARG;
  anyxboxx::Log("[d3d12_xs] D3D12ConfigureVirtualMemoryX : drapeaux %u, pages %u",
                description->flags, description->page_table_pages);
  return S_OK;
}

}  // extern "C"
