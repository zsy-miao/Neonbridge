// d3d12_xs.dll d'AnyXboxX : traduction de D3D12.X (DirectX 12 de la console).
//
// Première étape : les fonctions dont le contrat est identique sur PC sont
// redirigées vers le d3d12.dll de Windows par l'éditeur de liens (aucun code).
// Sont concernées les fonctions de sérialisation des signatures racine, qui
// n'échangent que des structures publiques et des blobs ID3DBlob.
//
// Pas encore traduit : la création du périphérique (D3D12XboxCreateDevice) et
// les interfaces ID3D12* de la console, dont la table de méthodes diffère de
// celle du PC ; les rediriger telles quelles ferait planter le jeu.
#include <windows.h>

#define ANYXBOXX_FORWARD_TO_D3D12(name) __pragma(comment(linker, "/export:" #name "=d3d12." #name))

ANYXBOXX_FORWARD_TO_D3D12(D3D12SerializeRootSignature)
ANYXBOXX_FORWARD_TO_D3D12(D3D12SerializeVersionedRootSignature)
ANYXBOXX_FORWARD_TO_D3D12(D3D12CreateRootSignatureDeserializer)
ANYXBOXX_FORWARD_TO_D3D12(D3D12CreateVersionedRootSignatureDeserializer)
