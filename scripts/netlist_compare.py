#!/usr/bin/env python3
"""Structural comparison of two gate-level netlists of strata_soc.

Compares the hierarchical yosys JSON netlist that the gate simulator runs
(build/gate/strata_soc.json) with a flat structural Verilog netlist written by
OpenROAD. Instance names are compared after flattening (hierarchy levels
joined with '/', as OpenROAD names them). Two netlists are structurally
identical when they have the same instances, each instance has the same cell
type, and every input pin (and every output port) is driven by the same
instance output pin, top-level input or constant.

  netlist_compare.py <gate.json> <flat.v>
"""
import json
import re
import sys
from pathlib import Path

CONST = {"0": ("const", "0"), "1": ("const", "1")}


def flatten_json(path: Path) -> tuple[dict, dict, dict]:
    """Returns (instances, drivers, directions):
    instances: name -> (type, {pin: net}); drivers: net -> key;
    directions: cell type -> {pin: 'input'|'output'}."""
    d = json.loads(path.read_text())
    mods = d["modules"]
    top = next(n for n, m in mods.items() if m.get("attributes", {}).get("top"))
    instances: dict[str, tuple[str, dict[str, object]]] = {}
    directions: dict[str, dict[str, str]] = {}
    top_ports: list[tuple[str, str, object]] = []  # (name, direction, net)
    counter = [0]
    parent: dict[int, int] = {}

    def fresh() -> int:
        counter[0] += 1
        parent[counter[0]] = counter[0]
        return counter[0]

    def find(x):
        if not isinstance(x, int):
            return x
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def walk(mname: str, prefix: str, bitmap: dict) -> None:
        m = mods[mname]

        def net(b):
            if isinstance(b, str):
                return CONST.get(b, ("const", "x"))
            if b not in bitmap:
                bitmap[b] = fresh()
            return bitmap[b]

        for cname, c in m.get("cells", {}).items():
            t = c["type"]
            if t in mods and not mods[t].get("attributes", {}).get("blackbox"):
                child = mods[t]
                cmap: dict = {}
                for pname, bits in c["connections"].items():
                    for cb, pb in zip(child["ports"][pname]["bits"], bits):
                        outer = net(pb)
                        if cb in cmap and isinstance(cmap[cb], int) and isinstance(outer, int):
                            # One child net on several port bits: the outer nets are the same net.
                            parent[find(outer)] = find(cmap[cb])
                        else:
                            cmap[cb] = outer
                walk(t, f"{prefix}{cname}/", cmap)
            else:
                pins = {}
                for pname, bits in c["connections"].items():
                    for i, b in enumerate(bits):
                        pins[pname if len(bits) == 1 else f"{pname}[{i}]"] = net(b)
                instances[prefix + cname] = (t, pins)
                directions.setdefault(t, {}).update(c.get("port_directions", {}))

    bitmap: dict = {}
    for pname, p in mods[top]["ports"].items():
        for i, b in enumerate(p["bits"]):
            if isinstance(b, int) and b not in bitmap:
                bitmap[b] = fresh()
            n = bitmap[b] if isinstance(b, int) else CONST.get(b, ("const", "x"))
            top_ports.append((pname if len(p["bits"]) == 1 else f"{pname}[{i}]", p["direction"], n))
    walk(top, "", bitmap)
    instances = {n: (t, {p: find(v) for p, v in pins.items()}) for n, (t, pins) in instances.items()}
    top_ports = [(name, d, find(n)) for name, d, n in top_ports]
    return finish(instances, directions, top_ports)


def finish(instances, directions, top_ports):
    drivers: dict = {}
    for name, (t, pins) in instances.items():
        for pin, n in pins.items():
            if directions[t].get(pin.split("[")[0]) == "output":
                drivers[n] = ("pin", name, pin)
    outputs = {}
    for pname, direction, n in top_ports:
        if direction == "input":
            drivers[n] = ("port", pname)
        else:
            outputs[pname] = n
    return instances, drivers, outputs


ID = r"(?:\\\S+|[A-Za-z_][A-Za-z0-9_$]*)"
NET = rf"({ID})\s*(?:\[(\d+)\])?"


def parse_verilog(path: Path, directions: dict) -> tuple[dict, dict, dict]:
    text = path.read_text()
    m = re.search(r"\bmodule\s+\S+\s*\((.*?)\);(.*)endmodule", text, re.S)
    body = m.group(2)
    widths: dict[str, tuple[str, int]] = {}
    for dm in re.finditer(r"^\s*(input|output)\s+(?:\[(\d+):(\d+)\]\s+)?(\S+);", body, re.M):
        direction, msb, name = dm.group(1), dm.group(2), dm.group(4)
        widths[name] = (direction, int(msb) + 1 if msb else 1)
    nets: dict[str, int] = {}
    parent: dict[int, int] = {}

    def find(x):
        if not isinstance(x, int):
            return x
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def net(tok: str):
        tok = tok.strip()
        c = re.fullmatch(r"1'b([01])", tok)
        if c:
            return CONST[c.group(1)]
        nm = re.fullmatch(NET, tok)
        if not nm:
            raise ValueError(f"cannot parse net {tok!r}")
        base = nm.group(1).lstrip("\\")
        key = base if nm.group(2) is None else f"{base}[{nm.group(2)}]"
        if key not in nets:
            nets[key] = len(nets) + 1
            parent[nets[key]] = nets[key]
        return nets[key]

    top_ports = []
    for name, (direction, w) in widths.items():
        for i in range(w):
            key = name if w == 1 else f"{name}[{i}]"
            top_ports.append((key, direction, net(key)))
    instances = {}
    for im in re.finditer(rf"^\s*(sky130_fd_sc_hd__\w+)\s+({ID})\s*\((.*?)\);", body, re.M | re.S):
        t, iname, conns = im.group(1), im.group(2).lstrip("\\"), im.group(3)
        pins = {}
        for pm in re.finditer(r"\.(\w+)\s*\(\s*(.*?)\s*\)\s*(?:,|$)", conns, re.S):
            if pm.group(2):
                pins[pm.group(1)] = net(pm.group(2))
        instances[iname] = (t, pins)
    # Continuous assignments make two names one net.
    consts = {}
    for am in re.finditer(r"^\s*assign\s+(.+?)\s*=\s*(.+?);", body, re.M):
        a, b = net(am.group(1)), net(am.group(2))
        if isinstance(b, int):
            parent[find(a)] = find(b)
        else:
            consts[find(a)] = b
    canon = lambda n: consts.get(find(n), find(n))  # noqa: E731
    instances = {k: (t, {p: canon(n) for p, n in pins.items()}) for k, (t, pins) in instances.items()}
    top_ports = [(k, d, canon(n)) for k, d, n in top_ports]
    return finish(instances, directions, top_ports)


def driver_of(drivers: dict, n) -> tuple:
    if isinstance(n, tuple):
        return n
    return drivers.get(n, ("undriven",))


def compare(a, b) -> list[str]:
    (ia, da, oa), (ib, db, ob) = a, b
    problems = []
    only_a = sorted(set(ia) - set(ib))
    only_b = sorted(set(ib) - set(ia))
    # Cells whose outputs drive nothing (in this design: the cells that keep
    # the debugger's observation nets) may be removed by dead-logic
    # elimination; they are reported, not counted as differences.
    loads = {n for _, (t, pins) in ia.items() for p, n in pins.items() if da.get(n) != ("pin", _, p)}
    loads |= set(oa.values())
    removed = [n for n in only_a if all(v not in loads for p, v in ia[n][1].items() if da.get(v) == ("pin", n, p))]
    only_a = [n for n in only_a if n not in removed]
    if removed:
        print(f"note: {len(removed)} cells without fanout are absent from the second netlist: {removed}")
    if only_a:
        problems.append(f"{len(only_a)} instances only in the first netlist, e.g. {only_a[:5]}")
    if only_b:
        problems.append(f"{len(only_b)} instances only in the second netlist, e.g. {only_b[:5]}")
    for name in sorted(set(ia) & set(ib)):
        (ta, pa), (tb, pb) = ia[name], ib[name]
        if ta != tb:
            problems.append(f"{name}: type {ta} vs {tb}")
            continue
        for pin in sorted(set(pa) | set(pb)):
            if pin not in pa or pin not in pb:
                problems.append(f"{name}.{pin}: connected in only one netlist")
            elif driver_of(da, pa[pin]) != driver_of(db, pb[pin]) and not (
                    driver_of(da, pa[pin])[0] == "pin" and driver_of(da, pa[pin]) == ("pin", name, pin)):
                problems.append(f"{name}.{pin}: driven by {driver_of(da, pa[pin])} vs {driver_of(db, pb[pin])}")
    for port in sorted(set(oa) | set(ob)):
        if driver_of(da, oa.get(port)) != driver_of(db, ob.get(port)):
            problems.append(f"output {port}: driven by {driver_of(da, oa.get(port))} vs {driver_of(db, ob.get(port))}")
    return problems


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    a = flatten_json(Path(sys.argv[1]))
    directions = json_directions(Path(sys.argv[1]))
    b = parse_verilog(Path(sys.argv[2]), directions)
    problems = compare(a, b)
    print(f"{len(a[0])} and {len(b[0])} instances; {len(problems)} differences")
    for p in problems[:40]:
        print("  " + p)
    sys.exit(1 if problems else 0)


def json_directions(path: Path) -> dict:
    d = json.loads(path.read_text())
    out: dict = {}
    for m in d["modules"].values():
        for c in m.get("cells", {}).values():
            out.setdefault(c["type"], {}).update(c.get("port_directions", {}))
    return out


if __name__ == "__main__":
    main()
