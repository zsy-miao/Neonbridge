// kernelx.dll d'AnyXboxX : le noyau de la Xbox, traduit vers celui du PC.
//
// Les binaires console importent kernelx.dll à la place de kernel32.dll. Toutes les
// fonctions communes sont redirigées vers kernel32/kernelbase de Windows par
// l'éditeur de liens (kernelx_forwarders.inc, généré par tools/kernelx_exports.py).
// Les fonctions propres à la console s'ajouteront ici, au fur et à mesure que les
// rapports (`scarlett.py --lancer … --md`) les révèlent.
#include <windows.h>

#include "kernelx_forwarders.inc"
