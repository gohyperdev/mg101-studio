# ADR-0002: Rdzeń w Ruście, model Device Pack i architektura web-first

- Status: zaproponowany
- Data: 2026-07-05
- Decydenci: Maciek Ostaszewski
- Dokumenty powiązane: [ADR-0001](0001-agent-tool-registry.md),
  [HLD platformy SaaS](../architecture/hld-saas-platform.md),
  [HLD integracji agentowej](../architecture/hld-agent-integration.md)

## Kontekst

MG101 Studio jest dziś natywną aplikacją macOS (Swift/SwiftUI, ~6,5 tys.
linii) z zaimplementowaną integracją agentową (ADR-0001). Nowe cele
produktowe wykraczają poza tę formę:

1. **Przenośność**: Windows i Linux jako pełnoprawne platformy.
2. **SaaS multitenant**: biblioteka patchy w chmurze, współdzielenie,
   biblioteka centralna, rozliczany agent-as-a-service.
3. **Multi-device**: docelowo kolejne multiefekty NUX oraz urządzenia
   innych producentów (inne formaty binarne, inne dialekty SysEx, inne
   możliwości sprzętowe). Fokus operacyjny pozostaje na NUX MG-101.
4. **Agent-first**: dostęp agentowy (czat + MCP lokalny i zdalny) jest
   pierwszoplanowym interfejsem, nie dodatkiem.

Wymóg krytyczny: logika domenowa (format, walidacja, rewizje, diff) musi
działać w trzech środowiskach — na serwerze (SaaS), na desktopie
(offline + device-link) i w przeglądarce (edycja bez round-tripu).

## Rozważane opcje

### A. Flutter (pełny port)

- Zalety: jeden codebase UI, znajomość w zespole, ścieżka mobile.
- Wady: brak oficjalnego MCP SDK; MIDI/SysEx na Windows/Linux słabo
  wspierane; logika domenowa w Darcie nie działa na serwerze inaczej niż
  przez osobny backend w innym języku; brak WASM-owej ścieżki współdzielenia
  rdzenia z przeglądarką.

### B. Kotlin Multiplatform + Compose Desktop

- Zalety: `javax.sound.midi`/`javax.sound.sampled` cross-platform
  z pudełka; jeden język zarządzany; rdzeń współdzielony z backendem JVM.
- Wady: dystrybucja z JRE; brak oficjalnego MCP SDK; brak ścieżki
  przeglądarkowej dla rdzenia; jakość ALSA na Linuksie nierówna.

### C. JUCE (C++)

- Zalety: branżowy standard edytorów sprzętu muzycznego (QuickTone sam
  jest w JUCE); najlepsze MIDI/audio.
- Wady: C++ (koszt rozwoju i bezpieczeństwa przy pracy na bajtach); UI
  wolne w iterowaniu; MCP/HTTP/multitenant do ręcznego sklejenia; brak
  ścieżki serwerowej i przeglądarkowej dla współdzielonego rdzenia.

### D. Rust core + web-first UI + Tauri (WYBRANA)

Rdzeń domenowy jako crate Rust kompilowany natywnie (serwer, desktop)
i do WASM (przeglądarka, sandboksowane kodeki urządzeń). Backend w Ruście
(axum), jeden frontend webowy dla SaaS i dla powłoki desktopowej (Tauri),
device-link przez `midir`/`cpal` na desktopie i WebMIDI (SysEx) w
Chrome/Edge.

- Zalety: jeden rdzeń we wszystkich trzech środowiskach; oficjalny MCP SDK
  (`rmcp`); najlepsza poza C++ cross-platformowa para MIDI/audio; język
  stworzony do bezpiecznej pracy na formatach binarnych; kodeki urządzeń
  jako moduły WASM dystrybuowane bez aktualizacji aplikacji; małe binarki.
- Wady: nowy język w stacku zespołu; UI w technologii webowej; okres
  przejściowy z podwójnym życiem Swift/Rust.

### E. Wyłącznie PWA (web-only)

- Zalety: zero instalacji; WebMIDI z SysEx wystarcza do transferu patchy.
- Wady: brak lokalnego MCP, brak przechwytywania audio, Safari/Firefox bez
  WebMIDI, ograniczony dostęp do plików. Odrzucona jako ścieżka jedyna;
  zawarta w D jako tryb przeglądarkowy tej samej aplikacji webowej.

## Decyzja

Wybieramy **opcję D**. Kluczowe rozstrzygnięcia:

1. **Model kanoniczny w rdzeniu.** Rdzeń nie zna MG-101: operuje na
   kanonicznym modelu patcha (łańcuch bloków → aktywny model → parametry
   z zakresami, pola globalne, załączniki typu IR). Biblioteka, rewizje,
   WAL, diff, sharing i definicje narzędzi agenta są pisane raz, przeciw
   modelowi kanonicznemu.
2. **Device Pack jako jednostka wsparcia urządzenia.** Wersjonowany pakiet:
   `manifest` (tożsamość, rodzina, firmware, capabilities), `profile`
   (dane: bloki/modele/parametry — dzisiejsze device-profile.json +
   effects-catalog.json), `codec` (kod: format binarny ↔ model kanoniczny),
   `protocol` (kod: dialekt SysEx, mapa CC, transfer, tryby USB audio).
   Profile są danymi; codec i protocol są kodem kompilowanym do WASM,
   podpisywanym i dystrybuowanym z centralnego rejestru.
3. **Bajty są święte.** Patch jest przechowywany zawsze jako oryginalny
   blob (nieznane bajty zachowane — kontynuacja zasady z wersji 1.2) plus
   zdekodowany model kanoniczny do wyszukiwania i edycji, z etykietą
   `device_id + firmware + wersja codeca`.
4. **Rewizje bez zmian koncepcyjnych.** Model `revision`/`expectedRevision`
   z ADR-0001 staje się mechanizmem synchronizacji lokalne↔chmura
   i wykrywania konfliktów multitenant.
5. **Narzędzia agenta generowane z profilu** — nigdy ręcznie per
   urządzenie. Dodanie urządzenia nie zmienia definicji narzędzi.
6. **MCP dwupoziomowo**: lokalny (desktop, Streamable HTTP na localhost)
   i zdalny per tenant (OAuth) w SaaS.
7. **Aplikacja Swift staje się implementacją referencyjną** do czasu
   domknięcia portu rdzenia (testy round-trip na istniejących plikach);
   potem jest wygaszana. `mg101-patch-tools` (spec YAML, mapa CC, runbooki)
   staje się metodologią autorstwa Device Packów.
8. **Poza zakresem platformy bazowej**: tone matching / przenoszenie
   brzmień między urządzeniami.

## Konsekwencje

- Powstaje workspace Rust (`core`, `device-pack-api`, `pack-nux-mg101`,
  `server`, `desktop`, `web`) — szczegóły i fazy w HLD platformy SaaS.
- Zakres rośnie z aplikacji do platformy: backend multitenant niesie
  obowiązki bezpieczeństwa, backupów i zgodności (RODO) — to główny koszt
  decyzji, nie technologia.
- Zespół przyjmuje Rusta do stacku; okres przejściowy Swift/Rust
  minimalizowany przez szybkie domknięcie portu rdzenia.
- WebMIDI ogranicza tryb przeglądarkowy do Chrome/Edge — komunikowane
  jawnie; pełnię możliwości daje desktop (Tauri).

## Historia

- v1 (2026-07-05): wersja pierwotna po analizie wariantów Flutter /
  Kotlin+Compose / JUCE / Rust+Tauri / PWA i doprecyzowaniu celów
  (Windows+Linux, SaaS multitenant, multi-device wielu producentów).
