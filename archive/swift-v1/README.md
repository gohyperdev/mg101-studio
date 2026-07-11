# NUX MG-101 Studio — Swift v1 (zarchiwizowany)

To jest **oryginalna implementacja Swift/SwiftUI** aplikacji NUX MG-101 Studio,
**wygaszona (E9)** po osiągnięciu parytetu przez przepisanie w Rust (`/rust`).

## Status: ZARCHIWIZOWANY — nie budować, nie rozwijać

Kod pozostaje w repozytorium (przeniesiony przez `git mv`, pełna historia
zachowana) **wyłącznie jako referencja** — źródło prawdy dla parytetu portu i
punkt odniesienia przy ewentualnych testach różnicowych. Nie jest już budowany
ani wydawany.

Aktywna implementacja: **`/rust`** (workspace Cargo, 11 crate'ów — rdzeń
device-agnostyczny, biblioteka, transfer, agent, MCP rmcp, desktop Slint).

## Co zostało przeniesione tu z korzenia repo

- `Package.swift`, `Package.resolved` — manifest SwiftPM.
- `Sources/` — `MG101Core`, `MG101Tools`, `MG101Studio` (SwiftUI), `MG101MCP`.
- `Tests/` — testy Swift.
- `Resources/Info.plist`, `scripts/build-app.sh` — pakowanie aplikacji macOS.

## Parytet i migracja danych

- **Bajty święte:** kodek Rust jest bezstratny bit-w-bit; oracle
  `Sources/MG101Core/Resources/factory-patches.mg101patch` został skopiowany do
  `rust/crates/pack-nux-mg101/oracle/` jako fixture bramki round-trip (36/36
  rekordów, 0 różnic). Format pliku `.mg101patch` jest w pełni kompatybilny —
  patche z v1 wczytują się w v2 bez konwersji.
- **Dziennik WAL** v2 jest natywny (nie migruje zawieszonych transakcji v1 —
  jest efemerycznym stanem crash-recovery; szczegóły w `rust/BACKLOG.md`).
- **Ustawienia AI** (klucz w Keychain) nie są automatycznie migrowane — do
  wprowadzenia ponownie w ustawieniach v2 (odnotowane w backlogu).

## Przywrócenie (gdyby potrzebne)

`git mv` zachował historię — przywrócenie to odwrotny ruch:
`git mv archive/swift-v1/{Package.swift,Package.resolved,Sources,Tests,Resources,scripts} .`
