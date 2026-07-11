# HLD: Platforma multi-device SaaS (rdzeń Rust + Device Packs)

- Status: projekt (high-level design), v1
- Data: 2026-07-05
- Decyzja bazowa: [ADR-0002](../adr/0002-rust-core-device-packs.md)
- Kontekst agentowy: [HLD integracji agentowej](hld-agent-integration.md)
  (architektura narzędzi/WAL/sesji przenosi się koncepcyjnie 1:1)

## 1. Cel

Platforma do edycji, przechowywania i współdzielenia patchy multiefektów:
początkowo NUX MG-101, docelowo kolejne urządzenia NUX i innych
producentów. Formy uruchomienia: SaaS (web, multitenant), desktop
(Windows/Linux/macOS, offline + device-link), przeglądarka (edycja +
WebMIDI). Dostęp agentowy (czat + MCP lokalny i zdalny) jako interfejs
pierwszoplanowy.

## 2. Zasady projektowe

1. **Jeden rdzeń, trzy środowiska** — logika domenowa w crate Rust,
   kompilowana natywnie (serwer, desktop) i do WASM (przeglądarka).
2. **Model kanoniczny** — rdzeń nie zna żadnego urządzenia; semantykę
   wnoszą Device Packi.
3. **Bajty są święte** — oryginalny blob patcha nigdy nie jest tracony ani
   nadpisywany; edycja przechodzi przez codec z testem round-trip.
4. **Profile to dane, codec/protocol to kod** (WASM, podpisany,
   z rejestru).
5. **Rewizje wszędzie** — `revision`/`expectedRevision` jako jeden
   mechanizm: konflikt UI↔agent, sync desktop↔chmura, współbieżność
   multitenant.
6. **Narzędzia agenta generowane z profilu** — jedna definicja dla
   wszystkich urządzeń i kanałów (czat, MCP lokalny, MCP zdalny).

## 3. Architektura

```
                        ┌────────────── Rejestr Device Packs ─────────────┐
                        │  podpisane paczki: manifest+profile+codec.wasm  │
                        │  +protocol.wasm; wersjonowanie per firmware     │
                        └───────┬─────────────────┬───────────────────────┘
                                ▼                 ▼
┌──────────── Backend SaaS ──────────┐   ┌────────── Klienci ─────────────┐
│ axum (Rust) · Postgres · S3-blob   │   │ Web UI (jeden frontend):       │
│ auth/OIDC · tenanty · ACL          │   │  • SaaS w przeglądarce         │
│ biblioteki: osobista/zespołu/      │◄──┤    (+ WebMIDI: Chrome/Edge)    │
│  centralna · sharing · wyszukiwanie│   │  • Tauri desktop: ten sam      │
│ AgentService (pętla tool-use,      │   │    frontend + midir/cpal       │
│  klucze per tenant, limity/koszty) │   │    (MIDI/SysEx/audio), pliki   │
│ MCP zdalny per tenant (OAuth)      │   │    lokalne, MCP lokalny, sync  │
│ Pack Runtime (WASM sandbox)        │   │    offline                     │
└──────────────┬─────────────────────┘   └──────────────┬─────────────────┘
               │              współdzielony rdzeń       │
               ▼                                        ▼
        ┌──────────────────────────────────────────────────────┐
        │ core (Rust): model kanoniczny · biblioteka · rewizje │
        │ WAL/transakcje · diff · sesje agenta · walidacja     │
        │ device-pack-api (traity Codec/Protocol/Capabilities) │
        └──────────────────────────────────────────────────────┘
```

### 3.1 Workspace Rust

| Crate | Rola | Kompilacja |
| --- | --- | --- |
| `core` | model kanoniczny, biblioteka, rewizje, WAL, diff, sesje | natywnie + WASM |
| `device-pack-api` | traity `PatchCodec`, `DeviceProtocol`, typy manifestu i capabilities | natywnie + WASM |
| `pack-nux-mg101` | pierwszy pack: codec 8402 B, protokół CC/SysEx MG-101 | natywnie + WASM |
| `server` | axum: REST + MCP zdalny + AgentService + Pack Runtime | natywnie |
| `desktop` | Tauri: device-link (midir/cpal), pliki, MCP lokalny, sync | natywnie |
| `web` | frontend (jeden dla SaaS i Tauri); core przez WASM | WASM |

`pack-nux-mg101` jest na starcie linkowany statycznie; kontrakt traitów
i pakowanie WASM obowiązują od pierwszego dnia, żeby drugi pack nie
wymagał przebudowy platformy.

### 3.2 Device Pack

```
manifest.toml   id, nazwa, producent, rodzina, obsługiwane firmware,
                capabilities: { cc_control, sysex_transfer, usb_audio_modes,
                ir_slots, patch_slots, looper, drums, ... }
profile/        bloki, modele, parametry, zakresy, pola globalne (dane)
codec.wasm      decode(blob) -> CanonicalPatch + niezdekodowane zakresy
                encode(CanonicalPatch, base_blob) -> blob
protocol.wasm   mapa CC, ramki SysEx (transfer patchy, stan), procedury
                (select_preset, push_patch, pull_patch, render_sample)
```

- **Kontrakt publikacji**: testy round-trip (decode→encode = identyczne
  bajty) na korpusie plików referencyjnych packa; raport pól nieznanych.
- **Podpis i wersjonowanie**: pack podpisany kluczem rejestru; wersja
  codeca zapisywana przy każdym patchu, który nim zdekodowano; zmiana
  firmware urządzenia → nowa wersja packa, stare patche pozostają czytelne
  starym codeciem.
- **Metodologia autorstwa**: proces przejścia MG-101 (reverse engineering
  → spec YAML → codec → testy round-trip → mapa CC/SysEx) z
  `mg101-patch-tools` staje się runbookiem onboardingu kolejnych urządzeń.

### 3.3 Model kanoniczny (szkic)

```rust
struct CanonicalPatch {
    device: DeviceRef,          // pack id + firmware + wersja codeca
    name: String,
    tempo: Option<Bpm>,
    chain: Vec<BlockState>,     // block_id, model_id, bypassed,
                                // params: Vec<(param_id, i32)>
    globals: BTreeMap<FieldId, i32>,
    attachments: Vec<Attachment>, // np. IR: nazwa + dane
    undecoded: Vec<ByteRange>,  // jawnie: czego codec nie rozumie
}
```

`undecoded` jest jawne w modelu — UI i agent widzą, że fragment patcha
jest poza semantyką codeca (zamiast udawać pełne zrozumienie formatu).

### 3.4 Model danych SaaS (rdzeń schematu)

| Encja | Kluczowe pola |
| --- | --- |
| `tenant` | organizacja lub konto osobiste |
| `user`, `membership` | OIDC, role per tenant |
| `library` | typ: personal / team / central; właściciel |
| `patch` | id, library_id, device_id, **blob** (S3), canonical (JSONB), firmware, codec_version, revision, lineage (fork-of), created_by |
| `patch_revision` | historia: blob + canonical + autor + źródło (ui/agent/sync/import) |
| `share` | patch/kolekcja → zakres (link, tenant, publiczny/centralny), uprawnienia |
| `agent_session` | odpowiednik SessionStore: messages.jsonl w blob, journal, liczniki tokenów/kosztów per tenant |
| `device_pack` | rejestr: manifest, wersje, podpisy, status publikacji |

Biblioteka centralna = `library(type=central)` kuratorowana per urządzenie;
sharing patcha to fork z zachowanym `lineage` (widać pochodzenie i wersję
źródła).

### 3.5 Synchronizacja desktop ↔ chmura

- Offline-first na desktopie: lokalna biblioteka (ta sama struktura
  z `core`), kolejka zmian.
- Push: `expectedRevision` jak w narzędziach agenta — konflikt zwraca
  aktualną rewizję; rozwiązanie po stronie klienta (obie wersje jako
  rewizje, wybór/duplikacja — bez cichego scalania bajtów).
- Pull: rewizje przyrostowo per biblioteka.
- Ten sam mechanizm obsługuje edycję z dwóch urządzeń tego samego
  użytkownika i pracę zespołową.

### 3.6 Warstwa agentowa

- **AgentService na backendzie**: pętla tool-use per sesja/tenant (klucze
  API po stronie usługi, limity i rozliczenia per tenant — liczniki
  `usage` z odpowiedzi providerów × cennik, jak w projekcie monitoringu
  kosztów dla aplikacji desktopowej).
- **Narzędzia**: te same definicje co w HLD integracji agentowej
  (read/write/filesystem/destructive), generowane z profilu aktywnego
  urządzenia; wariant biblioteczny (`patchID`+`expectedRevision`).
- **MCP zdalny per tenant** (Streamable HTTP + OAuth): użytkownik podpina
  Claude Code/Desktop do własnej biblioteki chmurowej.
- **MCP lokalny na desktopie**: jak w HLD agentowym (localhost + token).
- Desktop może działać z własnym kluczem użytkownika (BYOK) offline.

### 3.7 Device-link

| Zdolność | Przeglądarka (Chrome/Edge) | Desktop (Tauri) |
| --- | --- | --- |
| CC / zmiana parametrów na żywo | ✅ WebMIDI (SysEx) | ✅ midir |
| Transfer patchy (SysEx) | ✅ | ✅ |
| Przechwytywanie audio / próbki patchy | ❌ (brak dostępu do strumienia USB klasy audio per-device w praktyce) | ✅ cpal + procedura `render_sample` z packa |
| Praca offline / pliki lokalne | ograniczona | ✅ |

Procedury device-link są częścią `protocol.wasm` packa — platforma wywołuje
`push_patch`/`render_sample`, nie zna dialektu SysEx.

## 4. Fazy wdrożenia

| Faza | Zakres | Kryterium wyjścia |
| --- | --- | --- |
| R0 | Workspace + port rdzenia: `core`, `device-pack-api`, `pack-nux-mg101` (natywnie + WASM) | round-trip 100% na korpusie repo (`MG101AllPatch`, patche Fools Gold/Nirvana, factory 36); Swift = referencja porównawcza |
| R1 | Backend MVP: auth, tenant, biblioteka osobista, patch CRUD + rewizje, REST | pierwszy klient = testy + Claude przez MCP zdalny (read-only) |
| R2 | Web UI: biblioteka, edytor bloków, diff; WebMIDI (CC + transfer) | edycja patcha w przeglądarce end-to-end na żywym MG-101 |
| R3 | AgentService + czat w web UI + MCP zdalny z mutacjami (WAL, revert sesji) | parytet funkcji agenta z aplikacją Swift |
| R4 | Tauri desktop: ten sam frontend + midir/cpal + sync offline + MCP lokalny | praca offline i `render_sample` na Windows i Linux |
| R5 | Sharing, biblioteka centralna, rejestr packów (podpisy, publikacja) | drugi Device Pack opublikowany bez zmian w platformie |

Po R4 aplikacja Swift przechodzi w tryb utrzymaniowy; po R5 — archiwum.

## 5. Ryzyka i mitygacje

| Ryzyko | Mitygacja |
| --- | --- |
| Zakres „platformy" (security, backup, RODO) przerasta zespół | R1–R3 na jednym tenancie zamkniętym (własny zespół); publiczny SaaS jako świadoma decyzja po R5 |
| Rust nowy w zespole | rdzeń jest małą, dobrze przetestowaną domeną (port istniejącej logiki, nie eksploracja); UI pozostaje webowe |
| Codec WASM za wolny / za ciasny sandbox | codec MG-101 to proste operacje bajtowe; benchmark w R0, fallback: pack natywny na serwerze, WASM tylko w przeglądarce |
| WebMIDI tylko Chrome/Edge | jawna komunikacja; desktop domyka lukę |
| Dwa światy Swift/Rust w przejściu | R0 krótkie; zamrożenie funkcji w Swift poza poprawkami |
| Drugi pack ujawnia złe abstrakcje pack-api | próbka „papierowa" drugiego urządzenia (spec formatu innego NUX-a) jako test projektu traitów już w R0 |
| Koszty agenta per tenant | liczniki usage + limity budżetowe per tenant od R3 (projekt monitoringu kosztów przenosi się wprost) |

## 6. Otwarte pytania

1. Nazwa produktu/platformy (crate'y celowo neutralne).
2. Podpisywanie packów: własny klucz rejestru czy sigstore.
3. Polityka wersjonowania codeców przy aktualizacjach firmware NUX
   (obserwacja: format 8402 B stabilny między 1.x).
4. Granica core/pack dla funkcji „globalnych" urządzenia (looper, drums,
   tuner) — capabilities w manifeście vs narzędzia agenta per pack.
5. Licencjonowanie treści biblioteki centralnej (patche użytkowników).
