#!/usr/bin/env python3
"""Ekstraktor obrazów modeli z QuickTone (JUCE) → katalog obrazów MG-101.

QuickTone osadza grafiki modeli jako JUCE `BinaryData` (`temp_binary_data_N`).
Powiązanie nazwa→blob idzie przez switch po haszu JUCE w `getNamedResource`
(hash = 31*h + c). Identyfikatory obrazów (`Amp_Jazz_Clean_png`) pochodzą z
osadzonego JSON layoutu (`"image":"..."`). Ten skrypt:
  1. czyta symbole `nm` → offsety blobów,
  2. deasembluje `getNamedResource` (`otool -tV`, cache) → mapa hash→(blob,rozmiar),
  3. zbiera identyfikatory obrazów z JSON,
  4. dla każdego modelu katalogu MG-101 dobiera obraz po znormalizowanej nazwie,
  5. zapisuje `<moduł>_<slug>.png` do katalogu wyjściowego.

UWAGA IP: obrazy to własność NUX/Cherub. Skrypt (nasz kod) trafia do repo;
WYNIK (obrazy) świadomie NIE jest commitowany — patrz `.gitignore`. Domyślny
cel to katalog danych aplikacji, skąd desktop ładuje je w runtime.

Użycie:
  python3 tools/extract_quicktone_images.py \
      [--app /ścieżka/QuickTone.app] [--out KATALOG] [--catalog PLIK.json]
"""
from __future__ import annotations

import argparse
import json
import os
import re
import struct
import subprocess
import sys

JUCE_MASK = 0xFFFFFFFF
MACHO_TEXT_BASE = 0x100000000


def juce_hash(s: str) -> int:
    h = 0
    for c in s.encode():
        h = (31 * h + c) & JUCE_MASK
    return h


def symbol_offsets(binary: str) -> dict[int, int]:
    """Mapa indeks bloba → adres wirtualny (`temp_binary_data_N`)."""
    nm = subprocess.check_output(["nm", binary]).decode("latin1")
    syms: dict[int, int] = {}
    pat = re.compile(r"([0-9a-f]+) s __ZN10BinaryDataL?\d*temp_binary_data_(\d+)E")
    for line in nm.splitlines():
        m = pat.match(line)
        if m:
            syms[int(m.group(2))] = int(m.group(1), 16)
    return syms


def disassembly(binary: str, cache: str) -> list[str]:
    """`otool -tV` (deasembler) z cache — pełny disasm 178 MB binarki jest wolny."""
    if os.path.exists(cache) and os.path.getsize(cache) > 0:
        return open(cache, "r", errors="ignore").read().splitlines()
    print("Deasemblacja (otool -tV) — to potrwa…", file=sys.stderr)
    asm = subprocess.check_output(["otool", "-tV", binary]).decode("latin1")
    open(cache, "w").write(asm)
    return asm.splitlines()


def hash_to_blob(asm: list[str]) -> dict[int, tuple[int, int]]:
    """Mapa hash → (indeks bloba, rozmiar) z bloków `leaq data_N; movl $size`."""
    instr: list[tuple[int, str]] = []
    for line in asm:
        m = re.match(r"^([0-9a-f]{8,})\t(.*)$", line)
        if m:
            instr.append((int(m.group(1), 16), m.group(2)))
    block_at: dict[int, tuple[int, int]] = {}
    for k, (a, t) in enumerate(instr):
        mm = re.search(r"temp_binary_data_(\d+)E\(", t)
        if "leaq" in t and mm:
            idx = int(mm.group(1))
            size = None
            for kk in range(k + 1, min(k + 4, len(instr))):
                m2 = re.search(r"movl\t\$0x([0-9a-f]+), %ecx", instr[kk][1])
                if m2:
                    size = int(m2.group(1), 16)
                    break
            block_at[a] = (idx, size or 0)
    out: dict[int, tuple[int, int]] = {}
    for k, (a, t) in enumerate(instr):
        m = re.search(r"cmpl\t\$0x([0-9a-f]+), %eax", t)
        if not m:
            continue
        h = int(m.group(1), 16)
        if k + 1 >= len(instr):
            continue
        m2 = re.search(r"j(e|ne)\t0x([0-9a-f]+)", instr[k + 1][1])
        if not m2:
            continue
        tgt = int(m2.group(2), 16)
        if m2.group(1) == "e" and tgt in block_at:
            out[h] = block_at[tgt]
        elif m2.group(1) == "ne":
            for kk in range(k + 2, min(k + 5, len(instr))):
                if instr[kk][0] in block_at:
                    out[h] = block_at[instr[kk][0]]
                    break
    return out


def image_identifiers(data: bytes) -> set[str]:
    ids: set[str] = set()
    for pat in (rb'"image[A-Za-z]*"\s*:\s*"([A-Za-z0-9_]+)"', rb'"icon"\s*:\s*"([A-Za-z0-9_]+)"'):
        for m in re.findall(pat, data):
            if m:
                ids.add(m.decode())
    return ids


def json_model_map(data: bytes) -> dict[tuple[str, int], str]:
    """Mapa (moduł, model_id) → identyfikator obrazu z JSON layoutu QuickTone.

    Klucze JSON to `"N. DISPLAY NAME" : { "image": "Mod_Name_png" }` — N to
    `model_id`, a moduł wynika z prefiksu identyfikatora obrazu. To najpewniejsze
    powiązanie (bez zgadywania po nazwie); pokrywa gł. wzmacniacze."""
    out: dict[tuple[str, int], str] = {}
    pat = re.compile(
        rb'"(\d+)\.\s*[^"]+"\s*:\s*\{[^}]*?"image"\s*:\s*"([A-Za-z0-9_]+)"', re.S
    )
    for n, img in pat.findall(data):
        ident = img.decode()
        module = ident.split("_", 1)[0].lower()
        out.setdefault((module, int(n)), ident)
    return out


def norm_key(module: str, name: str) -> str:
    """Klucz dopasowania: moduł + nazwa (lower, bez `_png`, bez znaków spec.)."""
    name = re.sub(r"_png$", "", name)
    return f"{module.lower()}::{re.sub(r'[^a-z0-9]', '', name.lower())}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--app", default="/Users/mos/sources/nux/QuickTone.app")
    ap.add_argument("--catalog", default=os.path.join(
        os.path.dirname(__file__), "..", "rust", "crates", "pack-nux-mg101",
        "resources", "effects-catalog.json"))
    ap.add_argument("--out", default=os.path.expanduser(
        "~/Library/Application Support/dev.mos.mg101studio/model-images"))
    ap.add_argument("--cache", default="/tmp/quicktone-getnamed.asm")
    args = ap.parse_args()

    binary = os.path.join(args.app, "Contents", "MacOS", "QuickTone")
    if not os.path.exists(binary):
        print(f"Brak binarki QuickTone: {binary}", file=sys.stderr)
        return 1
    data = open(binary, "rb").read()
    syms = symbol_offsets(binary)
    addr_sorted = sorted(syms.values())

    def blob(idx: int) -> tuple[int, int]:
        a = syms[idx]
        nxt = min([x for x in addr_sorted if x > a], default=a)
        return a - MACHO_TEXT_BASE, nxt - a

    h2b = hash_to_blob(disassembly(binary, args.cache))
    print(f"switch cases (hash→blob): {len(h2b)}")
    ids = image_identifiers(data)
    resolved = {name: h2b[juce_hash(name)] for name in ids if juce_hash(name) in h2b}
    print(f"identyfikatory→blob: {len(resolved)}/{len(ids)}")

    # Indeks identyfikatorów po znormalizowanym kluczu (moduł z prefiksu nazwy).
    by_key: dict[str, str] = {}
    for name in resolved:
        parts = name.split("_", 1)
        if len(parts) == 2:
            by_key.setdefault(norm_key(parts[0], parts[1]), name)
    # Pewniejsze: mapa (moduł, model_id) → identyfikator z JSON layoutu.
    by_model = json_model_map(data)

    catalog = json.load(open(os.path.normpath(args.catalog)))
    os.makedirs(args.out, exist_ok=True)
    written = 0
    missing: list[str] = []
    for module, mod in catalog["modules"].items():
        for mid, model in mod.get("models", {}).items():
            slug = model.get("slug", "")
            display = model.get("display_name", "")
            # 1) JSON po model_id (najpewniejsze); 2) fallback po nazwie/slugu.
            cand = by_model.get((module, int(mid)))
            if cand not in resolved:
                cand = None
            if not cand:
                for probe in (norm_key(module, slug), norm_key(module, display)):
                    if probe in by_key:
                        cand = by_key[probe]
                        break
            if not cand:
                missing.append(f"{module}/{slug}")
                continue
            idx, _sz = resolved[cand]
            off, real = blob(idx)
            if data[off:off + 4] != b"\x89PNG":
                missing.append(f"{module}/{slug} (nie-PNG)")
                continue
            # Nazwa po (moduł, model_id) — UI dobiera obraz z block+model_id.
            dst = os.path.join(args.out, f"{module}_{mid}.png")
            open(dst, "wb").write(data[off:off + real])
            written += 1
    print(f"Zapisano {written} obrazów do {args.out}")
    if missing:
        print(f"Bez dopasowania ({len(missing)}): {', '.join(missing[:20])}"
              + (" …" if len(missing) > 20 else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
