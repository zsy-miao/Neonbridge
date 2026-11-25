"""Checked methods in the Xbox D3D12 interface layouts identified in Frosty's build."""
import json


def interface_inventory(evidence):
    path = evidence / "matrix-awakens/d3d12-xs-interfaces.json"
    if not path.is_file():
        return {}
    inventory = json.loads(path.read_text())
    sources = inventory["sources"]
    for source in sources.values():
        if not (evidence / source).is_file():
            raise ValueError(f"Missing D3D12 interface evidence: {source}")
    profile = json.loads((evidence / sources["profile"]).read_text())
    if (profile["package_version"], profile["executable_sha256"]) != (
        inventory["package_version"], inventory["title_sha256"]
    ):
        raise ValueError("D3D12 interface inventory belongs to a different Frosty build")

    registered = inventory["registered_slots"]
    seen = set()
    for interface in inventory["interfaces"]:
        for method in interface["slots"]:
            key = interface["name"], method["slot"]
            if key in seen or method["slot"] < 0 or method["identified"] not in sources:
                raise ValueError(f"Invalid D3D12 interface entry: {key}")
            checks = method.get("checks", {})
            if (not isinstance(checks, dict)
                    or any(source not in sources or not isinstance(scope, str) or not scope.strip()
                           for source, scope in checks.items())):
                raise ValueError(f"Invalid D3D12 interface check: {key}")
            if checks and method["slot"] not in registered.get(interface.get("table", interface["name"]), ()):
                raise ValueError(f"Checked D3D12 method has no registered handler: {key}")
            seen.add(key)
    return inventory


def d3d12_table(evidence):
    data = interface_inventory(evidence)
    if not data:
        return "No Xbox D3D12 interface measurements."
    lines = [f"Frosty {data['package_version']} · d3d12_xs interface methods",
             "Counts cover distinct slots in the catalogued interfaces. Each check has a limited test scope."]
    for entry in data["interfaces"]:
        slots = entry["slots"]
        checked = sum(bool(slot.get("checks")) for slot in slots)
        lines.extend(["", f"{entry['name']} · {checked}/{len(slots)} catalogued slots have TuxBox checks"])
        for slot in slots:
            result = ". ".join(slot.get("checks", {}).values()) or "No TuxBox check recorded"
            lines.append(f"  {slot['slot']:>3}  {slot['method']:<29}  {result}")
    lines.extend(["", "Registered slots in compiled TuxBox proxy tables (includes inherited methods)"])
    for name, slots in data["registered_slots"].items():
        lines.append(f"  {name:<23}  {len(slots):>2}   " + ", ".join(map(str, slots)))
    return "\n".join(lines)
