"""Table des slots du périphérique D3D12.X mesurés sur l'exécutable 1.0.2.1 (note de premier démarrage, W5).

La table compare, pour chaque slot que le titre appelle avant le premier « Close » en échec, ce que TomBox
assigne dans sa table de périphérique (crates/compat/d3d12_xs, 90 slots, DEVICE_SLOTS) et ce qu'AnyXboxX sert.
AnyXboxX ne sert aucun slot de périphérique : d3d12_xs.def n'exporte ni D3D12XboxCreateDevice ni de table.
Le test fige cet état : toute nouvelle implémentation doit mettre à jour la table et la note.
"""
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
DEF_FILE = ROOT / "runtime" / "d3d12_xs" / "d3d12_xs.def"
TOMBOX_DEVICE_SLOTS = 90  # DEVICE_SLOTS dans TomBox crates/compat/d3d12_xs/src/console_slots.rs

# slot, rôle mesuré (note, section 4), statut dans TomBox (grep des affectations `[n] = ...`), servi par AnyXboxX
DEVICE_SLOTS = (
    (8, "CreateCommandQueue (forme PC)", "assigné : create_command_queue_x (device_wrap.rs)", False),
    (9, "CreateCommandAllocator", "assigné : create_command_allocator_x (device_wrap.rs)", False),
    (16, "CreateRootSignature (forme)", "assigné : create_root_signature (cmdlist_validate.rs:1218, contexte non vérifié)", False),
    (26, "non établi (appelé avant le premier Close)", "non assigné au périphérique (26 = fake_barrier, cl_ext.rs:771, table de liste)", False),
    (36, "CreateFence (forme PC)", "non assigné", False),
    (41, "CreateCommandSignature", "assigné : create_command_signature_x (device_wrap.rs)", False),
    (48, "CreateCommandQueueX (console)", "assigné : create_command_queue_x (device_wrap.rs)", False),
    (49, "non établi", "non assigné", False),
    (57, "DeserializeGraphicsPipelineState", "assigné : deserialize_gfx_pso_x (device_wrap.rs)", False),
    (61, "CreateSampler", "assigné : create_sampler_x (device_wrap.rs)", False),
    (73, "CreateCommandListX (console)", "assigné : create_command_list_x (device_wrap.rs)", False),
    (74, "RegisterPagePool", "assigné : register_page_pool_x (device_wrap.rs)", False),
    (79, "ScheduleFrameEvent", "assigné : schedule_frame_event_x (device_wrap.rs)", False),
    (86, "non établi", "non assigné", False),
    (127, "CreateCommittedResource3 (forme)", "hors table : 127 >= 90 slots de périphérique", False),
)

MEASURED_SLOTS = {8, 9, 16, 26, 36, 41, 48, 49, 57, 61, 73, 74, 79, 86, 127}


def _exports():
    lines = DEF_FILE.read_text(encoding="utf-8").splitlines()
    start = lines.index("EXPORTS") + 1
    return {line.split()[0] for line in lines[start:] if line.strip() and not line.lstrip().startswith(";")}


def test_every_measured_slot_is_in_the_table():
    assert {slot for slot, *_ in DEVICE_SLOTS} == MEASURED_SLOTS


def test_slots_beyond_the_tombox_table_are_marked_as_such():
    beyond = {slot for slot, _, tombox, _ in DEVICE_SLOTS if slot >= TOMBOX_DEVICE_SLOTS}
    assert beyond == {127}
    assert all("hors table" in tombox for slot, _, tombox, _ in DEVICE_SLOTS if slot in beyond)


def test_anyxboxx_serves_no_device_slot():
    assert not any(served for *_, served in DEVICE_SLOTS)


def test_d3d12_xs_exports_no_device_creation_yet():
    # Sans D3D12XboxCreateDevice, le titre s'arrête avant la table (note, section 3, étape 12).
    assert "D3D12XboxCreateDevice" not in _exports()
    assert _exports() >= {"D3D12ConfigureVirtualMemoryX"}


@pytest.mark.parametrize("slot", sorted(MEASURED_SLOTS))
def test_tombox_status_is_recorded_for_each_slot(slot):
    rows = [tombox for s, _, tombox, _ in DEVICE_SLOTS if s == slot]
    assert len(rows) == 1 and rows[0]
