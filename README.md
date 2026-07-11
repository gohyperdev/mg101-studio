# MG101 Studio

> **Aktywna implementacja: `rust/`** — projekt został przepisany ze Swifta na
> Rust (workspace Cargo, 11 crate'ów: rdzeń device-agnostyczny, biblioteka,
> transfer, agent, serwer MCP rmcp, desktop Slint na macOS+Windows, rdzeń gotowy
> na WASM). Wersja Swift 1.x została **zarchiwizowana** w
> [`archive/swift-v1/`](archive/swift-v1/) (referencja, nie rozwijana). Format
> pliku `.mg101patch` jest w pełni kompatybilny — kodek Rust jest bezstratny
> bit-w-bit (bramka round-trip 36/36, 0 różnic). Build i testy: `cd rust && cargo
> test`. Szczegóły architektury: `docs/architecture/`, `docs/adr/`.

Poniższy opis dotyczy zachowania produktu (parytet utrzymany w wersji Rust).

Natywna aplikacja do bezpiecznej edycji plików NUX MG-101. Interfejs, writer i
serwer MCP korzystają z tego samego profilu urządzenia. Nieznane bajty pozostają
bez zmian, a eksport zawsze tworzy nowy plik.

## Zakres wersji 1.2

- biblioteka bez limitu 36 pozycji, startująca z 36 fabrycznymi patchami;
- trwałe przechowywanie zaimportowanych i edytowanych patchy w Application Support;
- wielokrotny import pojedynczych plików `.mg101patch`;
- import kompletnego zestawu QuickTone: 302472 bajty, 36 rekordów;
- eksport pojedynczego patcha oraz wybierany eksport dokładnie 36 patchy do zestawu;
- odczyt i eksport rekordów `.mg101patch` o długości 8402 bajtów;
- edycja nazwy, BPM, send/return, bypassów, modeli i parametrów;
- model-aware writer z walidacją zakresów i zerowaniem nieaktywnych parametrów;
- osadzanie i usuwanie lokalnego IR w formacie urządzenia;
- podgląd różnic bajtowych oraz undo/redo;
- podgląd binarny w układzie offset + hex + ASCII, z zaznaczeniem zmienionych wierszy;
- wymienny profil JSON bez ponownej kompilacji aplikacji;
- agent AI w aplikacji: plan zmian, walidacja, podgląd i jawne zatwierdzenie;
- serwer MCP oparty o oficjalny Swift SDK, uruchamiany przez stdio.

Aplikacja pracuje na plikach. Wersja 1.2 nie komunikuje się bezpośrednio z
urządzeniem przez USB; import i eksport do urządzenia wykonuje QuickTone.

## Biblioteka i zestawy patchy

Lewy panel zawiera bibliotekę patchy i nie ma limitu 36 pozycji. Przy pierwszym
uruchomieniu dostępnych jest 36 fabrycznych konfiguracji z potwierdzonego
zestawu MG-101. Importowane patche są przechowywane w:

```text
~/Library/Application Support/MG101Studio/PatchLibrary/
```

Widoczne przyciski pod biblioteką obsługują:

- **Import Files** — wybór jednego lub wielu plików; pojedynczy patch i pełny
  zestaw są rozpoznawane po rozmiarze;
- **Import Set** — import jednego pełnego pliku 36-patchowego;
- **Export Patch** — eksport aktualnie wybranego patcha;
- **Export Set** — panel wyboru dokładnie 36 pozycji z całej biblioteki.

QuickTone przyjmuje zestaw o rozmiarze dokładnie 302472 bajtów. Eksporter bierze
wybrane patche w kolejności biblioteki, ustawia im indeksy slotów 0–35 i łączy
36 rekordów po 8402 bajty. Plik wynikowy ma nazwę
`MG101AllPatch.mg101patch`.

Zakładka **Binary** ma dwa tryby:

- **Logical** — pola pogrupowane jako nagłówek patcha, pola globalne, bloki,
  aktywne parametry modelu i lokalny IR;
- **Raw** — cały rekord w wierszach po 16 bajtów: offset, wartości i ASCII.

Przełącznik **Hex / Decimal** zmienia sposób prezentacji offsetów i wartości
liczbowych w obu trybach. Zakresy zawierające zmiany względem wersji bazowej są
wyróżnione.

## Budowanie (Rust — aktywna implementacja)

```sh
cd rust
cargo build --workspace          # rdzeń + biblioteka + agent + MCP + desktop
cargo run -p mg101-desktop       # aplikacja desktop (Slint)
cargo run -p mg101-mcp           # serwer MCP po stdio (rmcp)
```

Bramka jakości (jak w CI, macOS + Windows):

```sh
cd rust
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

<details><summary>Archiwalny build Swift v1 (zarchiwizowany, nie rozwijany)</summary>

```sh
cd archive/swift-v1
swift test
scripts/build-app.sh
open dist/MG101Studio.app
```

Skrypt buduje oba programy dla `arm64`, tworzy pakiet `.app`, wykonuje podpis
ad-hoc i sprawdza podpis.

</details>

## Konfiguracja profilu

Profil składa się z dwóch plików:

- `device-profile.json` — rozmiar rekordu, bloki, selektory, okna parametrów,
  BPM i pola nazwane;
- `effects-catalog.json` — modele, parametry aktywne dla modelu, offsety i
  dozwolone zakresy.

Wybierz **Profile → Import Profile Folder…** i wskaż katalog zawierający oba
pliki. Aplikacja waliduje wersję schematu, rozmiar rekordu, offsety, unikalność
bloków i modeli oraz zakresy parametrów. Dopiero poprawny profil jest kopiowany
do:

```text
~/Library/Application Support/MG101Studio/Profiles/active/
```

Ten sam aktywny profil ładuje GUI i `MG101MCP`. Polecenie **Use Bundled
Profile** usuwa override i wraca do profilu dostarczonego z aplikacją. Zmiana
profilu zamyka bieżący dokument, aby nie interpretować jednego pliku dwiema
mapami.

## Agent AI w aplikacji

Konfigurację otwiera się przez **MG101 Studio → Settings… → AI Providers** albo
przycisk **Configure AI providers…** w zakładce **Agent**. Można wybrać:

- **Anthropic** — natywne Messages API, domyślny endpoint
  `https://api.anthropic.com/v1/messages`;
- **OpenAI-compatible** — Chat Completions, domyślny endpoint
  `https://api.openai.com/v1/chat/completions`; obsługuje również lokalne
  serwery i bramki zgodne z tym formatem.

Dla każdego dostawcy konfiguruje się osobno endpoint, model i klucz API.
Endpointy, modele oraz wybrany dostawca są zapisane w `UserDefaults`. Klucze API
są zapisane jako hasła ogólne w macOS Keychain pod usługą
`dev.mos.mg101studio.ai`; nie trafiają do plików konfiguracyjnych ani
`UserDefaults`.

Integracja Anthropic wysyła `POST /v1/messages`, nagłówek `x-api-key` oraz
`anthropic-version: 2023-06-01`. Model otrzymuje bieżący stan patcha oraz
aktualny katalog i może zaproponować operacje:

- `set_name`, `set_bpm`, `set_named_field`;
- `set_bypass`, `set_model`, `set_parameter`.

Odpowiedź modelu nie modyfikuje pliku bezpośrednio. Jest dekodowana do typowanych
operacji, walidowana przez core i wyświetlana do zatwierdzenia. Bez
skonfigurowanego endpointu działa prosty lokalny planner dla nazwy, BPM i
bypassów.

## MCP

Konfiguracja klienta MCP:

```json
{
  "mcpServers": {
    "mg101-studio": {
      "command": "/pełna/ścieżka/MG101Studio.app/Contents/MacOS/MG101MCP"
    }
  }
}
```

Dostępne narzędzia:

- `inspect_patch`, `list_models`;
- `set_parameter`, `set_bypass`, `set_model`;
- `set_bpm`, `set_name`, `set_named_field`;
- `set_ir`, `clear_ir`.

Każda operacja zapisująca wymaga osobnej ścieżki wejściowej i wyjściowej.
Istniejący plik nigdy nie jest nadpisywany. Zapis odbywa się przez plik
tymczasowy i atomowe przeniesienie do nowej ścieżki.

## Weryfikacja

```sh
cd rust
cargo test --workspace           # 178 testów (rdzeń, biblioteka, transfer, agent, MCP, desktop)
cargo run -p mg101-pack-nux-mg101 --example roundtrip_proof   # round-trip 1:1 36/36, 0 różnic
```

Smoke MCP: uruchom `cargo run -p mg101-mcp` i wykonaj sesję JSON-RPC po stdio
(initialize → tools/list → tools/call). Serwer neguje protokół, listuje narzędzia
wariantu plikowego i tworzy kopię patcha bez modyfikacji pliku źródłowego
(idempotentna odmowa nadpisania).

## Status projektu

Projekt jest nieoficjalny i nie jest powiązany z Cherub Technology ani marką
NUX. Nazwy produktów i znaków towarowych należą do ich właścicieli. Przed
importem do urządzenia zachowaj kopię oryginalnego zestawu patchy.
