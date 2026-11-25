// Adaptateur D3D12 choisi par son nom (ANYXBOXX_GPU) et description de l'adaptateur utilisé (--adaptateur).
#pragma once

#include <windows.h>
#include <d3d12.h>
#include <wrl/client.h>

#include <string>

// Vrai si ANYXBOXX_GPU contient autre chose que « warp » (quelle que soit la casse) ; `filter` reçoit sa valeur.
bool NamedAdapterRequested(std::wstring& filter);

// Périphérique sur le premier adaptateur matériel dont le nom contient `filter`, sans tenir compte de la casse.
// Aucun, ou périphérique impossible : message sur stderr et nullptr, jamais WARP.
Microsoft::WRL::ComPtr<ID3D12Device> CreateNamedDevice(const std::wstring& filter);

// --adaptateur : décrit en cinq lignes clé=valeur (UTF-8) l'adaptateur que CreateDevice choisirait.
int PrintAdapter();
