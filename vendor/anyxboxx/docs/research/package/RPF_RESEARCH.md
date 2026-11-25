> Français : [RPF_RECHERCHE.md](../../fr/recherche/paquet/RPF_RECHERCHE.md)

# RPF header survey

The 22 RPF files declared in the layout start with the ASCII bytes `9FPR`. Read as a 32-bit little-endian integer, they give `0x52504639`. The values below describe the observed bytes. They do not yet define a reader for the internal directory.

To examine an external header or an internal candidate without reading the whole file:

```powershell
python .\inspect_rpf.py .\script.rpf
python .\inspect_rpf.py .\script.rpf --offset 0x1000
python .\inspect_rpf.py .\pack\story000\story000_script.rpf --offset 0x18000
```

The program prints the 16 little-endian words and the SHA-256 fingerprint of the 64 examined bytes. The offset is counted from the start of the file.

| Offset | Observations on the 22 external headers |
| --- | --- |
| 0x00 | `0x52504639` |
| 0x04, 0x08, 0x0c | Variable values. For the 22 files, `0x04` and `0x08` are multiples of 16 and `0x04 > 0x08`. Their exact meaning needs confirmation. |
| 0x10, 0x14 | Variable values. |
| 0x18, 0x1c | `0x00047a00` and `0x00000150` respectively. |
| 0x20, 0x24, 0x28 | Variable values. |
| 0x2c | `0x00090f01` or `0x00090f02`. |
| 0x30 | `0x00000010`. |
| 0x34 to 0x3c | Three zero words. |

The aligned search in the two script files found three candidates in `script.rpf`, the first at offset zero, and five in `story000_script.rpf`, the first at zero. The eight internal and external candidates of the two files share the constant table values at offsets 0x18, 0x1c and 0x30. In the two external headers, the word at 0x24 is 2 and 4 respectively. These values match the number of internal candidates found. This correlation on two examples remains a hypothesis. It does not allow concluding that the field is the general entry count.

Bytes that reproduce a signature can occur inside data. The detected positions are therefore candidates to examine. The exact limits of the internal archives, the entry names and their content remain to be determined.
