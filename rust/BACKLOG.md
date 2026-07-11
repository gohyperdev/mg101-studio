# Backlog techniczny (uwagi z review, per epik)

## Do E1
- `device-pack-api`: `StorageLimits` uzupełnić o `per_bank` (HLD §2); `BankInfo`
  dodać `erasable`. Kontrakt musi pokryć profil JSON MG-101.
- `device-pack-api`: `DeviceProtocol` dodać `read_bank` (bulk dump — capability
  MG-101, AC E2).

## Do E2 (przed pracą na żywym sprzęcie)
- `device-link` `SysexAssembler` (odziedziczone z probe — wierność zachowana):
  domknąć obsługę realtime (`F8–FF`) wplecionego w SysEx i między bajtami
  running-status (nie kasować częściowego CC), oraz przerwanie sklejania SysEx
  przy statusie ≠F0/F7 (ochrona przed niekończącym się buforem). Dodać testy.

## Nice-to-have (dowolny moment)
- CI: cache `Swatinem/rust-cache` + `concurrency` group.
- `Cargo.toml`: usunąć redundantne `[lib] name/path`; zweryfikować URL repo.
- E6: zdecydować, czy `mcp` to osobna binarka czy embed w desktop.
