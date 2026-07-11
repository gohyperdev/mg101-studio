# Backlog techniczny (uwagi z review, per epik)

## Do E1 (przeniesione dalej — kontrakt device)
- `device-pack-api`: `StorageLimits` uzupełnić o `per_bank` (HLD §2); `BankInfo`
  dodać `erasable`. Kontrakt musi pokryć profil JSON MG-101.
- `device-pack-api`: `DeviceProtocol` dodać `read_bank` (bulk dump — capability
  MG-101, AC E2).

## Z review E1 (niekrytyczne — do E2/E3)
- `canonical.rs`: `ir_present`/`ir_name` to pola pochodne (snapshot) — enkod
  używa `ir_region`. Udokumentować lub zamienić na akcesory.
- Nakładka offsetów 90–93 (`sr.params` vs `namedFields patch.*`): enkod pisze
  pola globalne po blokach → semantyka edycji kanonicznej do udokumentowania/
  deduplikacji.
- Offsety IR (`0x82/0x86/0xA6`) jako stałe MG-101 w rdzeniu — docelowo do
  profilu (region IR), by rdzeń był w pełni device-agnostyczny.
- Inwariant „profil musi przejść `validate()`": rozważyć typ `ValidatedProfile`
  zamiast bezpośredniego indeksowania (parytet ze Swiftem, ale panic możliwy).
- `patch_record.rs`: guard `record_size >= 0xA6` (underflow w `set_ir`).
- `Container::split` → własny enum błędu zamiast `String`; dodać `join`.
- Duplikacja `resources/*.json` vs `Sources/MG101Core/Resources/` — test/krok
  build porównujący albo jedno źródło (ryzyko dryfu).

## Do E2 (przed pracą na żywym sprzęcie)
- `device-link` `SysexAssembler` (odziedziczone z probe — wierność zachowana):
  domknąć obsługę realtime (`F8–FF`) wplecionego w SysEx i między bajtami
  running-status (nie kasować częściowego CC), oraz przerwanie sklejania SysEx
  przy statusie ≠F0/F7 (ochrona przed niekończącym się buforem). Dodać testy.

## Nice-to-have (dowolny moment)
- CI: cache `Swatinem/rust-cache` + `concurrency` group.
- `Cargo.toml`: usunąć redundantne `[lib] name/path`; zweryfikować URL repo.
- E6: zdecydować, czy `mcp` to osobna binarka czy embed w desktop.
