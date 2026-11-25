"""Extraction de la table API → (fournisseur, interface, méthode) depuis xgameruntime.lib."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

import gdk_interfaces  # noqa: E402

SAME_GUID = """
XTaskQueueCreate:
  0000000000000032: 48 8D 0D 00 00 00  lea         rcx,[_GUID_073b7dcb_1fcf_4030_94be_e3c9eb623428]
                    00
  0000000000000039: 48 8B D1           mov         rdx,rcx
  000000000000003C: E8 00 00 00 00     call        QueryApiImpl
  0000000000000047: 48 8B 4C 24 68     mov         rcx,qword ptr [rsp+68h]
  000000000000004C: 48 8B 01           mov         rax,qword ptr [rcx]
  0000000000000057: 48 8B 40 60        mov         rax,qword ptr [rax+60h]
  000000000000005B: E8 00 00 00 00     call        _guard_dispatch_icall
  000000000000006A: 48 8B 40 10        mov         rax,qword ptr [rax+10h]

XUserUnregisterForDeviceAssociationChanged:
  000000000000002E: 48 8D 15 00 00 00  lea         rdx,[_GUID_0cc6a956_e7e1_4fdf_9341_9d5da649ebc8]
  0000000000000035: 48 8D 0D 00 00 00  lea         rcx,[_GUID_7d824997_10dc_45ab_86b7_2737767c0bf1]
  000000000000003C: E8 00 00 00 00     call        QueryApiImpl
  0000000000000053: 48 8B 40 28        mov         rax,qword ptr [rax+28h]

XTaskQueueDispatch:
  0000000000000032: 48 8D 0D 00 00 00  lea         rcx,[_GUID_073b7dcb_1fcf_4030_94be_e3c9eb623428]
  0000000000000039: 48 8B D1           mov         rdx,rcx
  000000000000003C: E8 00 00 00 00     call        QueryApiImpl
  0000000000000057: 48 8B 40 60        mov         rax,qword ptr [rax+80h]

XHelperWithoutRuntime:
  0000000000000000: C3                 ret
"""


def test_reads_provider_interface_and_method():
    table = {api.name: api for api in gdk_interfaces.parse(SAME_GUID)}
    create = table["XTaskQueueCreate"]
    assert create.provider == create.interface == "073B7DCB-1FCF-4030-94BE-E3C9EB623428"
    assert create.method == 12
    user = table["XUserUnregisterForDeviceAssociationChanged"]
    assert user.provider == "7D824997-10DC-45AB-86B7-2737767C0BF1"
    assert user.interface == "0CC6A956-E7E1-4FDF-9341-9D5DA649EBC8"
    assert user.method == 5
    assert "XHelperWithoutRuntime" not in table


def test_include_file_is_a_cpp_x_macro_table():
    text = gdk_interfaces.to_include(gdk_interfaces.parse(SAME_GUID), "test")
    assert 'ANYXBOXX_GDK_API(XTaskQueueCreate, "073B7DCB-1FCF-4030-94BE-E3C9EB623428", ' \
           '"073B7DCB-1FCF-4030-94BE-E3C9EB623428", 12)' in text
    assert text.index("XTaskQueueCreate") < text.index("XUserUnregister")   # ordre stable : par nom


def test_groups_name_each_interface_by_its_functions():
    groups = gdk_interfaces.groups(gdk_interfaces.parse(SAME_GUID))
    assert groups[("073B7DCB-1FCF-4030-94BE-E3C9EB623428",) * 2] == "XTaskQueue"


def test_unreadable_query_is_reported():
    unreadable = []
    gdk_interfaces.parse("XOddFunction:\n  call        QueryApiImpl\n", unreadable)
    assert unreadable == ["XOddFunction"]


def test_versions_are_sorted_numerically():
    paths = ["C:/VS/14.9/dumpbin.exe", "C:/VS/14.10/dumpbin.exe"]
    assert max(paths, key=gdk_interfaces._version_key) == "C:/VS/14.10/dumpbin.exe"
