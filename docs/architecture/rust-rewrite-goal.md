# /goal — Przepisanie MG101 Studio na Rust (core + UI), z pełną funkcjonalnością agentową

> Treść do wprowadzenia w komendę `/goal`. Kompletny brief implementacyjny
> nowej wersji. Bazuje na: [ADR-0002](../adr/0002-rust-core-device-packs.md),
> [ADR-0001](../adr/0001-agent-tool-registry.md),
> [HLD Biblioteka/urządzenie/transfer](hld-library-device-sync.md),
> [HLD SaaS](hld-saas-platform.md) oraz na empirii sprzętowej
> `nux/mg101-probe/docs/findings.md` (zdekodowany protokół MG-101).

## 1. Cel i definicja sukcesu

Przepisać obecną aplikację macOS (Swift/SwiftUI, ~5,9 tys. linii, 4 targety) na
**Rust z czystym podziałem core ↔ UI**, zachowując **100% dzisiejszych
funkcjonalności** i dodając bezpośrednie sterowanie urządzeniem oraz model
Biblioteka↔Urządzenie. Wersja teraz: **macOS i Windows** (desktop). Architektura
**gotowa na wariant webowy** bez przepisywania rdzenia. Cała **funkcjonalność
agentowa** (LLM, narzędzia, MCP, sesje, WAL, autoryzacje) przeniesiona.

**Sukces =**
1. Rdzeń Rust przechodzi round-trip 1:1 na istniejących plikach `.mg101patch`
   (Swift jako oracle) — bajt w bajt, nieznane bajty zachowane.
2. Aplikacja desktop (macOS + Windows) z pełnym edytorem, biblioteką, agentem
   i MCP działa na równi z v1.
3. Bezpośredni link do MG-101: wierny zrzut 72 slotów + zapis pod kontrolą.
4. Model Biblioteka/urządzenie z transferem (pojedynczy/grupowy) i limitami.
5. Ten sam rdzeń kompiluje się do WASM (ścieżka web udowodniona testem, nawet
   jeśli UI web dostarczymy później).
6. Aplikacja Swift v1 wygaszona dopiero po osiągnięciu parytetu (nie wcześniej).

## 2. Architektura docelowa: core ↔ UI

**Zasada:** jeden **rdzeń bezstanowy względem platformy** + cienkie powłoki UI.
Wszystko, co nie jest rysowaniem pikseli, żyje w core. Komunikacja przez **jedną
magistralę komend** (Command/Query bus) — ten sam kontrakt obsługuje UI, agenta,
MCP i przyszły web.

```
                    ┌────────────────────────────────────────────┐
   UI desktop       │                  CORE (Rust)               │
  (Tauri: macOS,    │                                            │
   Windows)  ──IPC──┤  Command/Query API  ← jedno API dla wszystkich
                    │        │                                    │
   UI web (później) │        ├─ canonical model (patch/bloki/param)
   (WASM/HTTP) ─────┤        ├─ device-pack-api (traity, topologia)
                    │        ├─ pack-nux-mg101 (profile+codec+protocol)
   Agent (LLM) ─────┤        ├─ device-link (MIDI/SysEx: midir/WebMIDI)
   (te same komendy)│        ├─ library (store, tagi, grupy, provenance)
                    │        ├─ transfer-engine (push/pull/sync, limity)
   MCP klient ──────┤        ├─ agent-core (pętla tool-calling, providerzy)
  (rmcp, lokalny)   │        ├─ tools (generowane z profilu) + WAL/rewizje
                    │        └─ sessions (session/messages/journal)
                    └────────────────────────────────────────────┘
```

**Kluczowa unifikacja (zgodna z v1 i ją porządkująca):** dzisiejsze `DomainCommand`
+ rejestr narzędzi + most HTTP + MCP + IPC UI zbiegają się w **jeden rejestr
komend**. Każda operacja (edycja parametru, zmiana modelu, transfer, import…) to
komenda z:
- schematem argumentów (źródłem prawdy dla narzędzi agenta — generowane, nie ręczne),
- klasą (`read`/`write`/`filesystem`/`destructive`) sterującą autoryzacją,
- kontrolą rewizji (`expectedRevision`) i wpisem WAL dla operacji mutujących.

To samo API woła: przycisk w UI (Tauri IPC), agent (tool call), serwer MCP
(`rmcp`) i — docelowo — frontend web przez HTTP. **Zero logiki domenowej w UI.**

**Workspace Rust** (wg ADR-0002, doprecyzowany):
`core`, `device-pack-api`, `device-link`, `pack-nux-mg101`, `library`,
`transfer-engine`, `agent-core`, `mcp` (rmcp), `desktop` (Tauri), `web` (później),
`server` (SaaS, później). Dev-CLI: dzisiejszy `mg101-probe` jako narzędzie
diagnostyczne (`list/monitor/probe/dump`).

## 3. Zakres — pełna funkcjonalność v1 do przeniesienia (mapa, nic nie ginie)

### 3.1 Model domenowy i formaty (→ `core` + `pack-nux-mg101`)
- Format `.mg101patch`: pojedynczy (8402 B) i zestaw 36 slotów; bajty LE, offsety
  stałe. **Bajty święte** — nieznane zachowane, oryginalny blob + model kanoniczny.
- `PatchRecord` → kanonik + codec: slot index (0..3), nazwa (offset 109, 16 B),
  BPM (7-bit MSB/LSB 95/96, 40–300), selektor bloku (6 bitów model + bit 0x40
  bypass), IR (flaga 0x82, nazwa 0x86..0xA6 32 B, WAV od 0xA6 z walidacją RIFF/WAVE).
- Mutacje: setByte/BPM/Name/NamedField/Bypass/Parameter (range-check)/Model
  (+`clearInactive`); `differences(from:)` (silnik diff).
- `DeviceProfile` (JSON, schemaVersion): recordSize, 11 bloków (wah/cmp/efx/amp/
  eq/gate/mod/dly/rvb/cab/sr), selectorOffset+parameterOffsets, namedFields
  (send 87/return 88/patch.min 90/max 91/level 92/position 93), pełna walidacja.
- `EffectCatalog` (JSON, ~9,2 tys. linii): moduły→modele→parametry (local_index,
  file_offset, raw_range, semantic_confidence). Liczności modeli: wah 1, cmp 2,
  efx 14, amp 25, eq 2, gate 1, mod 14, dly 7, rvb 5, cab 27, sr 1.
- `ProfileLoader`: zasoby bundlowane + **aktywny override** profilu
  (`install(from:)`/`removeActiveOverride`) → w Rust: instalacja Device Packa.
- `PatchFileWriter` write-once (nigdy nie nadpisuje).

### 3.2 Biblioteka (→ `library`, rozszerzona wg HLD)
- Dzisiejsza `LibraryPatch` (id/patch/original/sourceName/origin/revision),
  `library-index.json`, pliki `<id>.mg101patch` + `.originals/`.
- Factory wirtualne (36, ładowane z bundla, materializowane po edycji →
  `.editedFactory`); migracja legacy (backup + seed + import + dedup).
- Import (pojedynczy/zestaw 36; panel/drag/`onOpenURL`/agent), export (pojedynczy;
  zestaw 36 z mapowaniem kolejność→sloty).
- **Nowe (HLD):** dowolny rozmiar, **tagi** (wiele-do-wielu) i **grupy**
  (uporządkowane kolekcje), fingerprint zawartości, provenance, stan trójdrożny.
  Store: SQLite (desktop) ze schematem gotowym na Postgres/sync (rewizje).

### 3.3 Edycja (→ `core` API + UI)
- Model selekcji: `selectedPatchID` (+ reset undo/redo), `selectedBlockID`.
- Edytor bloku: picker modelu (re-clamp parametrów), bypass, slidery parametrów
  (live, min/max, offset hex), kontrolki patcha (rename, BPM stepper, send/return,
  Load IR/Clear IR).
- Inspektor 4 zakładki: **Changes** (diff), **Binary** (hex/dec: Raw 16-B wiersze
  + ASCII, Logical: sekcje semantyczne, podświetlenie zmian), **Agent**, **MCP**.
- Undo/redo (stosy snapshotów per patch, ⌘Z/⇧⌘Z, bump rewizji + persist).

### 3.4 Agent / LLM (→ `agent-core`, pełna migracja)
- Dwaj providerzy: **Anthropic** (Messages API, `x-api-key`, `anthropic-version`,
  max_tokens, tools w formacie Anthropic) i **OpenAI-compatible** (Chat
  Completions, Bearer opcjonalny — Ollama/LM Studio, tools w formacie funkcji).
- Normalizacja endpointu (dokłada `/v1/messages` / `/chat/completions`).
- **Pętla tool-calling** (`AgentLoop`, max 20 iteracji): POST historii → parse
  text+tool_calls → wykonanie komend → wynik jako tura user → powtórka.
- System prompt (przeniesiony i uogólniony na model kanoniczny, nie „MG-101”).
- Strumień tekstu asystenta (per-tura; **do poprawy:** prawdziwy SSE token-level).
- **Autoryzacje:** filesystem (approval ścieżki, `approvedRoots` na sesji),
  destructive (zawsze potwierdzenie) — panel „Autoryzacja operacji”.
- `ChatMessage` (role/content/toolCalls/toolResults) → tłumaczenie na kształt
  każdego providera.
- **Dług do naprawienia:** `totalCost`/liczenie tokenów **zaimplementować
  naprawdę** (in/out tokeny, koszt sesji, rozmiar kontekstu — było pytanie
  użytkownika); usunąć martwy `LocalAgentPlanner`/`AgentAPIClient.plan`; naprawić
  bug `type:"type"` w payloadzie tool-call OpenAI.

### 3.5 Narzędzia agenta (→ `tools`, generowane z profilu)
Przenieść **wszystkie** (warianty `.library` = target po id+rewizji, `.file` =
ścieżki wej/wyj). Klasa steruje autoryzacją. Aliasy argumentów tolerowane
(`bool_value`↔`bypassed`, `parameter`↔`field`, `wav`↔`wavPath`).
- **read:** list_patches, get_patch, get_selection, list_models, get_profile,
  get_diff.
- **write:** set_parameter, set_model, set_bypass, set_name, set_bpm,
  set_named_field, clear_ir, duplicate_patch, revert_last_agent_action,
  select_patch.
- **filesystem:** set_ir, import_patch, export_patch, list_files.
- **destructive:** delete_patch (soft-delete/staging), revert_session.
- MCP-only: inspect_patch.
- **Nowe (Biblioteka/transfer):** tag/untag, create_group/add_to_group,
  push_to_device, pull_from_device, sync_device, list_device_slots — generowane
  z tego samego rejestru, klasa `write`/`destructive` wg skutku.

### 3.6 MCP (→ `mcp` na `rmcp`)
- Serwer MCP (dziś stdio, swift-sdk) → **`rmcp`**, transport stdio + Streamable
  HTTP na localhost (ADR-0002 §6). Eksponuje komendy z rejestru z anotacjami
  (`readOnlyHint`/`destructiveHint`/`openWorldHint`) z klasy komendy.
- Dzisiejszy most HTTP in-app (NWListener, port 10101, bearer token w
  `~/.mg101_bridge_*`) → zastąpiony lokalnym MCP z auth; **dostęp do żywej
  biblioteki**, nie tylko plikowy.
- `MCPInfoView` → ekran konfiguracji klienta (ścieżka/porty/token).

### 3.7 Trwałość (→ `sessions` + `library`)
- Sesje: `session.json` (metadane: provider/model/state active|committed|reverted/
  turnCount/totalCost/approvedRoots/**profileHash**/libraryPath), `messages.jsonl`
  (append-only), kompakcja (`keepLast + summary`) — **teraz wywoływana
  automatycznie** przy dużym kontekście.
- **WAL / journal** (`journal.jsonl`): dwufazowe wpisy (prepared→committed),
  inverse ops (restoreBytes/removePatch/restoreFromStaging), hashe SHA256
  before/after, revertSession/revertLastAgentAction, **crash recovery** przy
  starcie (prepared-bez-committed), soft-delete staging. Przenieść 1:1.
- Klucze API: macOS Keychain / Windows Credential Manager (przez abstrakcję
  `secret-store`), fallback env (`ANTHROPIC_API_KEY`/`OPENAI_API_KEY`).

### 3.8 Powłoka UI (→ `desktop` Tauri, macOS+Windows)
- Struktura: okno główne (lista patchy z badge origin + ikona IR, sygnałowy
  łańcuch bloków, edytor, inspektor) + ekran Ustawień (provider/endpoint/model/
  klucze) + ekran MCP.
- Komendy menu: Import (⌘O), Export patcha (⇧⌘S), Export zestawu, Import/Use
  Device Pack, Undo/Redo (⌘Z/⇧⌘Z).
- **Nowe:** trzy zakładki **User / Factory / Library** (HLD §6), drag patch/grupa
  → bank, import slotu → biblioteka, plan transferu + potwierdzenie + postęp.
- i18n: uporządkować mieszankę PL/EN (docelowo klucze tłumaczeń).

### 3.9 Nowość: bezpośredni link do urządzenia (→ `device-link` + pack `protocol`)
- Transport z `mg101-probe` (`SysexAssembler`, ramkowanie, stabilny `unique_id`)
  → `device-link` (midir natywnie, WebMIDI w web) za jednym traitem.
- MG-101 `protocol`: odczyt (`09/0B 00 <idx>`), zapis slotu (`0B 01 <slot> <189B>`),
  mapa CC (0–91), Program Change. Transkodowanie rekord 189 B ↔ kanonik ↔ plik.
- Read-only dump banku (testowalny od zaraz), zapis pod kontrolą (WAL + plan).
- Wniosek z capture 08: **jesteśmy hostem**, nie MITM dla QuickTone.

## 4. Stack i platformy
- **Rdzeń:** Rust (natywny + `wasm32` dla web). Testy round-trip przeciw plikom v1.
- **UI desktop:** Tauri 2 (macOS + Windows teraz), frontend web (React/Svelte —
  do ustalenia) współdzielony z przyszłym webem.
- **MIDI/SysEx:** `midir` (desktop), WebMIDI (web, Chrome/Edge).
- **Agent:** `agent-core` z providerami Anthropic/OpenAI-compatible; MCP `rmcp`.
- **Store:** SQLite (desktop) ze schematem pod Postgres/sync (SaaS później).
- **Sekrety:** Keychain (macOS) / Credential Manager (Windows) za abstrakcją.

## 5. Epiki / fazy (kamienie milowe + kryteria akceptacji)

- **E0 — Szkielet workspace.** Crates wg §2; wciągnięcie transportu z
  `mg101-probe` do `device-link`. AC: `cargo build` + CI (macOS+Windows).
- **E1 — Rdzeń + pack MG-101 (offline).** Kanonik, `device-pack-api`
  (+`DeviceStorage`), profil+codec MG-101. AC: **round-trip 1:1** na wszystkich
  istniejących `.mg101patch` i zestawie 36 vs Swift; nieznane bajty zachowane.
- **E2 — Link do sprzętu.** `device-link` + protocol; read-only dump 72 slotów;
  zapis pod kontrolą (WAL+plan). AC: wierny zrzut banku z urządzenia użytkownika.
- **E3 — Rejestr komend + narzędzia + WAL.** Jedno API, generacja narzędzi
  z profilu, rewizje, journal, crash recovery, staging. AC: testy portu WAL
  (append/recovery/revert/konflikt hash) zielone.
- **E4 — Biblioteka.** Store, tagi, grupy, fingerprint, provenance, stan
  trójdrożny, import/export (pojedynczy+zestaw), migracja. AC: parytet z v1 +
  tagi/grupy.
- **E5 — Silnik transferu.** push/pull/sync, transfer grupowy, limity, WAL. AC:
  wgranie grupy na User z limitami + cofnięcie; import banku do biblioteki.
- **E6 — Agent-core + MCP.** Providerzy, pętla tool-calling, autoryzacje, sesje,
  kompakcja, **liczenie tokenów/kosztu**, MCP `rmcp` (stdio+HTTP). AC: parytet
  agenta v1 + realny licznik kosztu; MCP z żywą biblioteką.
- **E7 — UI desktop (Tauri, macOS+Windows).** Edytor, inspektor (Changes/Binary/
  Agent/MCP), lista, ustawienia, trzy zakładki + transfer UI, menu, undo/redo,
  i18n. AC: parytet UX z v1 + nowe zakładki; działa na macOS i Windows.
- **E8 — Dowód WASM.** Rdzeń kompiluje się do `wasm32`; kodek round-trip w WASM.
  AC: test w WASM przechodzi (UI web dostarczany osobno/później).
- **E9 — Wygaszenie Swift v1.** Dopiero po parytecie E1–E7.
- **(E10+ — SaaS/chmura: osobny HLD.)**

## 6. Poza zakresem (teraz) i ryzyka
- Poza zakresem: transparentny MITM dla QuickTone (capture 08 — niewykonalny na
  CoreMIDI), tone-matching między urządzeniami, mobile, backend SaaS (późniejszy
  HLD), UI webowe (rdzeń ma być tylko gotowy).
- Ryzyka: nowy język w stacku (Rust) i okres podwójnego życia Swift/Rust; WebMIDI
  tylko Chrome/Edge; wybór frameworku frontendu; wierność zapisu na sprzęt
  (mitygacja: WAL + plan + dry-run + testy na realnym urządzeniu).

## 7. Naprawy długu z v1 (wbudowane w zakres)
1. Prawdziwe liczenie tokenów in/out, rozmiaru kontekstu i kosztu sesji.
2. Auto-kompakcja historii przy dużym kontekście.
3. Usunięcie martwego kodu (`PatchWorkspace` dublujący, `LocalAgentPlanner`,
   `AgentAPIClient.plan`).
4. Poprawka `type:"type"` w tool-call OpenAI.
5. Streaming token-level (SSE) zamiast per-tura.
6. Uporządkowanie i18n (PL/EN → klucze).
