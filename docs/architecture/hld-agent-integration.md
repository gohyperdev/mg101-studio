# HLD: Integracja agentowa MG101 Studio

- Status: projekt (high-level design), v3 — po drugiej recenzji architektonicznej
- Data: 2026-07-04
- Decyzja bazowa: [ADR-0001](../adr/0001-agent-tool-registry.md)

## 1. Cel

Umożliwić pełną manipulację danymi aplikacji (biblioteka patchy, aktywny
patch, IR, import/eksport) w formie dialogu z agentem LLM — zarówno
z czatu wewnątrz aplikacji, jak i (opcjonalnie) z zewnętrznego agenta przez
MCP — z rozmowami trwałymi między uruchomieniami aplikacji i bez naruszania
gwarancji edytora: walidacji profilem urządzenia, tworzenia nowych plików
zamiast nadpisywania oraz odwracalności zmian na poziomie sesji.

## 2. Stan obecny (punkt wyjścia)

| Element | Plik | Rola dziś |
| --- | --- | --- |
| One-shot agent | `Sources/MG101Studio/AgentAPIClient.swift` | prompt → JSON z operacjami, bez tool callingu |
| Zastosowanie planu | `StudioState.applyAgentProposal()` | walidacja i zatwierdzanie operacji |
| Fallback offline | `LocalAgentPlanner.swift` | heurystyki tekstowe |
| Stan aplikacji | `StudioState.swift` (`@MainActor`) | biblioteka, wybór, undo/redo, persystencja |
| Logika domenowa | `MG101Core` (PatchRecord, DeviceProfile, EffectCatalog) | walidowany zapis bajtów |
| MCP plikowy | `Sources/MG101MCP/main.swift` | 10 narzędzi na plikach, stdio |
| Sekrety | `KeychainStore.swift` | klucze API |

Znane ograniczenia istotne dla projektu:

- undo stack jest globalny i czyszczony przy zmianie zaznaczenia
  (`StudioState.selectedPatchID.didSet`) — nie obsłuży sesji agenta
  modyfikującej wiele patchy;
- identyfikatory patchy fabrycznych są losowane przy każdym uruchomieniu,
  a `origin`/`sourceName`/`original` nie są utrwalane (defekt — patrz
  faza 0);
- `StudioState` jest w całości `@MainActor` — przez MainActor muszą
  przechodzić **również odczyty**, nie tylko mutacje; wymiana profilu
  (`activateProfile`) przeładowuje bibliotekę i unieważnia identyfikatory;
- import, eksport i wybór IR otwierają `NSOpenPanel`/`NSSavePanel`
  bezpośrednio z metod `StudioState`.

## 3. Architektura docelowa

```
┌──────────────────────────── MG101Studio.app ────────────────────────────┐
│                                                                          │
│  AgentChatView ──► AgentLoop (poza MainActor) ──► Anthropic / OpenAI     │
│   (sesje rozmów,      │   ▲                                              │
│    streaming,         │   │ tool_result                                  │
│    Revert session)    ▼   │                                              │
│                  GUIToolExecutor ◄──────────────── MCPBridge (faza 4)    │
│                       │      komendy domenowe z MG101Tools   ▲           │
│    serializowany      ▼                                      │           │
│    (@MainActor)  StudioState + serwisy bezpanelowe           │           │
│                       │ mutacje przez                        │           │
│                       ▼ dziennik WAL (prepared→committed)    │           │
│                  MG101Core (walidacja)  SessionStore ────────┘           │
│                                         (rozmowy + dzienniki, App Supp.) │
└──────────────────────────────────────────────────────────────────────────┘
                                                        ▲
                                 zewnętrzni agenci (Claude Code, Desktop)

   MG101Tools (nowy target: komendy domenowe + typy operacji,
   zależny wyłącznie od MG101Core)
        ├── konsumowany przez MG101Studio (GUIToolExecutor)
        └── konsumowany przez MG101MCP (FileToolExecutor, stdio, pliki)
```

### 3.1 Moduł MG101Tools (definicje)

Nowy target SwiftPM zależny wyłącznie od `MG101Core` — bez AppKit, bez
`StudioState`. Zawiera:

```swift
struct ToolDefinition: Sendable {
    let name: String              // np. "set_parameter"
    let description: String
    let inputSchema: JSONValue    // JSON Schema argumentów
    let kind: ToolKind            // .read | .write | .filesystem | .destructive
}
```

#### Model tożsamości i współbieżności

```swift
typealias PatchID = String        // "factory-01"…"factory-36" lub UUID importu

struct PatchRef: Sendable {
    let patchID: PatchID
    let expectedRevision: Int     // optymistyczna kontrola współbieżności
}
```

- Każdy patch w bibliotece ma monotoniczny licznik `revision`,
  inkrementowany przy **każdej** mutacji — agentowej, ręcznej w GUI
  i zdalnej (MCPBridge). Rewizja jest utrwalana w indeksie biblioteki
  (faza 0).
- Wszystkie narzędzia mutujące przyjmują jawnie `patchID`
  + `expectedRevision`. Niezgodność rewizji → błąd `conflict` z aktualną
  rewizją w wyniku; agent odświeża odczyt (`get_patch`) i ponawia decyzję.
- **Globalne zaznaczenie UI nigdy nie jest warunkiem poprawności
  mutacji** — `select_patch` to wyłącznie operacja widoku (3.3).

#### Wspólna domena, osobne schematy transportowe

Wspólne dla wszystkich executorów są **komendy domenowe** i walidacja
argumentów (blok, model, zakresy — `DeviceProfile`/`EffectCatalog`)
w jednym miejscu. Schematy transportowe są generowane z tej samej komendy
w dwóch wariantach i **celowo nie są identyczne**:

- **wariant biblioteczny** (`patchID` + `expectedRevision`) — czat GUI
  i MCPBridge (faza 4);
- **wariant plikowy** (`input`/`output`, ścieżki bezwzględne) — plikowy
  `MG101MCP` (dzisiejsza semantyka: wejście → nowy plik wyjściowy).

Oba warianty mapowane są na formaty `tools` Anthropic, `functions` OpenAI
oraz `Tool` MCP; wyniki są zwięzłym JSON-em z limitem rozmiaru (kontrola
kosztu kontekstu). Testy kontraktowe pokrywają oba mapowania.

### 3.2 Executory (wykonanie)

- **GUIToolExecutor** (target `MG101Studio`): **jeden serializowany
  executor** na `@MainActor`, przez który przechodzą wszystkie operacje
  narzędziowe na stanie — odczyty i mutacje. Pętla agentowa nigdy nie
  czyta `StudioState` bezpośrednio; odczyty zwracają niemutowalną migawkę
  (wartości `Sendable`) pobraną na MainActor. Mutacje przechodzą przez
  dziennik WAL (3.5).
- **FileToolExecutor** (target `MG101MCP`): obecna semantyka plikowa;
  po migracji korzysta z komend domenowych `MG101Tools` (wariant plikowy)
  zamiast własnych schematów.

Polityka równoległości: w danej chwili mutować może jedna sesja agentowa;
mutacje użytkownika w GUI i innych źródeł są dozwolone, a ich kolizje
z sesją wykrywa `expectedRevision` (błąd `conflict` zamiast cichego
nadpisania).

Zasada: handlery nigdy nie otwierają paneli systemowych. Logika importu,
eksportu i osadzania IR zostaje wydzielona z `StudioState` do serwisów
przyjmujących gotowe URL-e (`PatchImportService`, `PatchExportService`,
`IRService`); metody panelowe w GUI jedynie pozyskują URL i delegują do
serwisu. Te same serwisy wołają handlery narzędzi.

### 3.3 Zestaw narzędzi

Odczyt (`.read`, wykonywane automatycznie, bez zatwierdzania):

| Narzędzie | Argumenty | Opis |
| --- | --- | --- |
| `list_patches` | — | biblioteka: stabilne `patchID`, `revision`, nazwa, slot, origin, czy zmodyfikowany |
| `get_patch` | `patchID` | pełny stan patcha (bloki, modele, parametry, named fields, IR, BPM) + `revision` |
| `get_selection` | — | aktualnie wybrany patch i blok (informacyjnie, dla kontekstu UI) |
| `list_models` | `block` | modele i zakresy parametrów dla bloku |
| `get_profile` | — | bloki, named fields, limity profilu |
| `get_diff` | `patchID` | różnice bajtowe patcha względem utrwalonego oryginału |

Mutacje stanu (`.write`, protokół WAL — 3.5; wszystkie przyjmują
`patchID` + `expectedRevision`):

| Narzędzie | Opis |
| --- | --- |
| `set_parameter`, `set_model`, `set_bypass` | edycja bloku wskazanego patcha |
| `set_name`, `set_bpm`, `set_named_field` | pola globalne wskazanego patcha |
| `clear_ir` | usunięcie osadzonego IR |
| `duplicate_patch` | kopia patcha w bibliotece (zwraca `patchID` kopii) |
| `revert_last_agent_action` | cofnięcie ostatniego zatwierdzonego wpisu dziennika bieżącej sesji |

Operacja UI (poza dziennikiem, bez wpływu na semantykę mutacji):

| Narzędzie | Opis |
| --- | --- |
| `select_patch` | synchronizacja widoku (podążanie zaznaczenia za pracą agenta); nie jest warunkiem ani składnikiem żadnej mutacji |

`undo`/`redo` **nie są wystawiane agentowi**: globalny stos UI jest
czyszczony przy zmianie zaznaczenia (`selectedPatchID.didSet`), więc
w sekwencji `select_patch → undo` byłby pusty lub cofałby nie to, czego
agent oczekuje. Odwracalność pracy agenta zapewnia wyłącznie dziennik
sesji (`revert_last_agent_action`, `revert_session`). Undo/redo pozostają
w UI dla edycji ręcznych, bez zmian.

Operacje plikowe (`.filesystem`, dodatkowo polityka ścieżek — 3.6):

| Narzędzie | Opis |
| --- | --- |
| `set_ir` | osadzenie IR z pliku WAV do wskazanego patcha (`patchID` + `expectedRevision`, ścieżka zatwierdzona) |
| `import_patch` | import pliku lub zestawu 36 patchy (ścieżka zatwierdzona) |
| `export_patch` | eksport wskazanego patcha do nowego pliku w zatwierdzonym katalogu; nigdy nadpisanie |

Destrukcyjne (`.destructive`, zawsze potwierdzenie w UI):

| Narzędzie | Opis |
| --- | --- |
| `delete_patch` | miękkie usunięcie patcha (`patchID` + `expectedRevision`; staging — 3.5), fabryczne chronione jak dziś |
| `revert_session` | cofnięcie całej sesji (odtworzenie dziennika od końca); dostępne też jako akcja UI |

### 3.4 AgentLoop

Pętla tool-use zastępująca one-shot:

1. Działa poza MainActor (sieć, streaming, parsowanie, budowa historii);
   operacje na stanie wyłącznie przez serializowany GUIToolExecutor (3.2).
2. Wywołanie API z definicjami narzędzi; odpowiedź `tool_use` → dispatch do
   executora → `tool_result` → kolejna tura; tekst → wiadomość dla
   użytkownika (streaming do UI).
3. Limit iteracji na turę użytkownika (np. 20) oraz timeout żądania.
4. Obsługa obu providerów (Anthropic tool use, OpenAI function calling) za
   wspólną abstrakcją; konfiguracja i klucze jak dotychczas
   (`AISettingsView`, `KeychainStore`).
5. Narzędzia `.destructive` i `.filesystem` spoza zatwierdzonych ścieżek
   zwracają do pętli status `needs_confirmation`; pętla wstrzymuje się do
   decyzji użytkownika w UI i kontynuuje z wynikiem zatwierdzenia/odmowy.
6. Błąd `conflict` (nieaktualna `expectedRevision`) wraca do agenta jako
   zwykły `tool_result` z aktualną rewizją — pętla nie jest przerywana.

### 3.5 Dziennik transakcji (WAL) i „Revert session"

Checkpoint oparty wyłącznie na migawkach bajtów patcha nie cofnie operacji
strukturalnych (import, duplikacja, usunięcie). Zamiast tego każda mutacja
sesji przechodzi protokół **write-ahead log**:

```
TransactionEntry {
    sequence, timestamp, toolName,
    patchID, revisionBefore,
    inverse: InverseOperation,   // np. .restoreBytes(id, blobRef),
                                 // .removePatch(id), .restoreFromStaging(id, meta)
    beforeHash: SHA256,          // hash stanu patcha PRZED operacją
    afterHash:  SHA256,          // hash stanu patcha PO operacji
    state: prepared | committed
}
```

Protokół odporny na awarię:

1. **prepared** — wpis z `beforeHash`, odwrotnością i argumentami trafia
   do `journal.jsonl` (append + fsync) **przed** wykonaniem mutacji;
2. mutacja wykonywana atomowo (zapis nowych bajtów do pliku tymczasowego
   + `rename`; operacje strukturalne analogicznie — najpierw materiał,
   potem podmiana indeksu);
3. **committed** — wpis domykany rekordem z `afterHash`.

**Recovery przy starcie**: wpisy `prepared` bez domknięcia są
rozstrzygane deterministycznie — jeśli bieżący stan patcha odpowiada
`beforeHash`, mutacja nie zaszła i wpis jest anulowany; w przeciwnym razie
stan przywracany jest odwrotnością. Aplikacja nigdy nie pracuje na
bibliotece niespójnej z dziennikiem.

Odwrotności per typ operacji:

- **Edycje patcha** → przywrócenie poprzednich bajtów (blob z `.originals/`
  lub zapisany w wpisie).
- **Import/duplikacja** → usunięcie dodanych pozycji.
- **Usunięcie** → wykonywane jako przeniesienie pliku do
  `PatchLibrary/.staging/<sessionID>/` z metadanymi; odwrotność:
  przywrócenie ze stagingu.
- **Eksport** → poza dziennikiem (nowy plik poza biblioteką; nigdy
  nadpisanie — gwarancja `PatchFileWriter.writeNew`).

Weryfikacja przy cofaniu:

- „Revert session" odtwarza dziennik od końca. Przed każdą odwrotnością
  porównuje bieżący stan patcha z **`afterHash`** wpisu — rozbieżność
  oznacza ręczną zmianę po operacji agenta (konflikt). Po wykonaniu
  odwrotności stan jest weryfikowany względem **`beforeHash`**. Przy
  konflikcie proces zatrzymuje się i raportuje, które wpisy cofnięto,
  a które wymagają decyzji użytkownika.
- `revert_last_agent_action` cofa pojedynczo ostatni `committed` wpis
  bieżącej sesji według tych samych reguł.

Maszyna stanów sesji i cykl życia stagingu:

```
            ┌──Commit session──► committed   (nieodwracalne; staging kasowany)
active ─────┤
            └──Revert session──► reverted    (dziennik odtworzony; staging przywrócony)
```

- Fizyczne kasowanie stagingu następuje **wyłącznie** po przejściu sesji
  w stan `committed` (jawne „Commit session" w UI albo polityka retencji
  sesji z 3.7). Ręczne „Empty trash" jest możliwe, ale UI ostrzega, że
  unieważnia revert dotkniętych wpisów.
- Zamknięcie okna czy restart aplikacji **nie zmienia stanu sesji** —
  sesja `active` pozostaje w pełni odwracalna po restarcie (dziennik
  i staging są trwałe, utrwalane razem z sesją — 3.7).
- Dziennik jest niezależny od globalnego undo stacku UI (ten pozostaje bez
  zmian dla edycji ręcznych i nie jest wystawiany agentowi — 3.3).

### 3.6 Polityka ścieżek dla narzędzi `.filesystem`

- Sesja agenta utrzymuje listę katalogów zatwierdzonych przez użytkownika
  (approved roots). Pierwsze użycie ścieżki spoza listy → `needs_confirmation`
  w UI z podglądem pełnej ścieżki; zatwierdzenie może obejmować katalog.
- Odczyt WAV/patchy i zapis eksportów wyłącznie w zatwierdzonych korzeniach;
  ścieżki są normalizowane (realpath) przed sprawdzeniem, symlinki
  rozwiązywane.
- Zatwierdzenia są zapisywane per sesja (nie globalnie); nowa rozmowa
  zaczyna z pustą listą.

### 3.7 Trwałość sesji rozmów (SessionStore)

Rozmowy z agentem przetrwają restart aplikacji:

- Magazyn: `Application Support/MG101Studio/AgentSessions/<uuid>/`
  - `session.json` — metadane (tytuł, daty, provider/model, **stan sesji**
    (`active`/`committed`/`reverted`), liczniki tur i kosztów, approved
    roots, **`profileHash`** aktywnego profilu i ścieżka biblioteki),
  - `messages.jsonl` — pełna historia: wiadomości użytkownika i agenta,
    wywołania narzędzi z argumentami i wynikami (append-only, zapis
    przyrostowy po każdej turze),
  - `journal.jsonl` — dziennik transakcji sesji; **zapisywany per mutacja
    zgodnie z protokołem WAL (3.5), nie per tura** — awaria w środku tury
    nie rozspójnia biblioteki z dziennikiem.
- **Wiązanie z profilem**: wymiana profilu urządzenia przy sesji `active`
  jest zablokowana — UI proponuje najpierw „Commit session" albo „Revert
  session". Wznowienie sesji przy niezgodnym `profileHash` wymaga jawnej
  decyzji użytkownika (kontynuacja tylko do odczytu / unieważnienie
  dziennika / powrót do zapisanego profilu).
- **Wznowienie**: lista sesji w UI → wybór → odtworzenie historii do
  kontekstu. Przy długich sesjach starsze tury są kompaktowane do
  podsumowania (stały prompt „summary of earlier work"), ostatnie N tur
  wchodzi dosłownie — kontrola kosztu kontekstu.
- Po wznowieniu agent nie ufa zapamiętanemu stanowi: pierwsza tura po
  wznowieniu zachęca (w prompcie systemowym) do odświeżenia odczytów
  (`list_patches`/`get_patch`), bo biblioteka mogła się zmienić między
  sesjami; `expectedRevision` i tak wychwyci pracę na nieaktualnym stanie.
- Retencja: konfigurowalny limit liczby sesji (np. 50); usunięcie sesji
  `active` z UI wymaga potwierdzenia i przenosi ją najpierw w stan
  `committed` (kasując staging); sekrety nigdy nie trafiają do plików
  sesji (klucze tylko w Keychain).
- Identyfikatory patchy w historii są stabilne dzięki fazie 0 — odwołania
  w starych rozmowach pozostają poprawne.

### 3.8 UI (AgentChatView)

- Zakładka Agent staje się widokiem rozmowy: lista sesji (wznów / nowa /
  usuń), historia, streaming, wpisy narzędziowe w formie kompaktowej
  („🔧 set_parameter amp.gain = 55 → OK"), błędy inline, dialogi
  potwierdzeń (`.destructive`, ścieżki).
- Pasek sesji: liczba tur/koszt, stan sesji, „Revert session",
  „Commit session", „Nowa rozmowa".
- `LocalAgentPlanner` pozostaje jako tryb offline (bez zmian).

### 3.9 MCPBridge (faza 4, opcjonalna)

- Definicje z `MG101Tools` (wariant biblioteczny) wystawione przez MCP
  Streamable HTTP na `localhost` (losowy port + token pokazywany
  w zakładce MCP; brak dostępu spoza hosta). GUI nie może być serwerem
  stdio — stdio wymaga uruchomienia procesu przez klienta.
- Zewnętrzny agent przechodzi przez **ten sam serializowany
  GUIToolExecutor i protokół rewizji**, więc obowiązują te same zasady:
  dziennik WAL, polityka ścieżek, potwierdzenia `.destructive` w oknie
  aplikacji, `conflict` przy nieaktualnej rewizji.
- Plikowy `MG101MCP` pozostaje do pracy wsadowej; po fazie 1 przechodzi na
  komendy domenowe z `MG101Tools` (wariant plikowy).

## 4. Plan wdrożenia

| Faza | Zakres | Szacunek |
| --- | --- | --- |
| 0 | **Bugfix biblioteki i model danych**: typ `PatchID` (String: `factory-01…36`, importy — UUID w formie tekstowej), indeks `library-index.json` (id, origin, sourceName, `revision`), utrwalenie oryginałów jako niemutowalne bloby w `PatchLibrary/.originals/<id>` (baza diffa i odwrotności — sam hash nie odtwarza bajtów), migracja istniejących plików z kopią zapasową; deduplikacja **wyłącznie** fabrycznych duplikatów powstałych z defektu (importy i celowe kopie użytkownika nietykane) | 2 dni |
| 1a | Target `MG101Tools`: komendy domenowe + walidacja + oba warianty schematów transportowych (biblioteczny, plikowy); testy kontraktowe mapowań (Anthropic/OpenAI/MCP × 2 warianty) | 1–2 dni |
| 1b | Serwisy bezpanelowe (`PatchImportService`, `PatchExportService`, `IRService`) + serializowany GUIToolExecutor (odczyty przez migawki, mutacje z `expectedRevision`, licznik rewizji w indeksie) | 1–2 dni |
| 1c | Dziennik WAL + staging + maszyna stanów sesji + recovery przy starcie; testy przerwań (crash między `prepared` a `committed`), konfliktów rewizji i konfliktów hashy przy revercie | 2 dni |
| 2 | `AgentLoop` poza MainActor (oba providery, streaming, `needs_confirmation`, obsługa `conflict`), SessionStore z wznowieniami i wiązaniem profilu | 2–3 dni |
| 3 | `AgentChatView` (sesje, rozmowa, wpisy narzędziowe, Revert/Commit session, potwierdzenia) | 1–2 dni |
| 4 | `MCPBridge` (Streamable HTTP + token), migracja `MG101MCP` na `MG101Tools`, dokumentacja w zakładce MCP | 1–2 dni |
| 5 | Narzędzia wyższego poziomu: kreacja patcha z opisu, porównania A/B, przyszły sync USB | wg potrzeb |

Suma faz 0–4: **9–13 dni**. Fazy 1a–1c mają osobne kryteria ukończenia
i mogą być dostarczane niezależnie (1a nie dotyka GUI, 1c nie zależy od
providerów).

Kryteria ukończenia faz 0–3:

- ID patchy, `revision` i metadane (origin, sourceName, oryginał) przeżywają
  restart; edycja patcha fabrycznego nie tworzy duplikatów plików;
- mutacja z nieaktualną `expectedRevision` zwraca `conflict` i **nie
  zmienia stanu**; równoległa edycja ręczna w GUI podczas sesji agenta
  jest wykrywana, nie nadpisywana;
- crash w dowolnym punkcie protokołu WAL (przed mutacją, w trakcie, po
  mutacji a przed `committed`) po restarcie kończy się biblioteką spójną
  z dziennikiem — test z symulowanym przerwaniem;
- agent potrafi w jednej rozmowie: wylistować bibliotekę, odczytać stan,
  zmienić parametry wielu patchy (bez polegania na zaznaczeniu),
  zaimportować plik z zatwierdzonego katalogu, zweryfikować odczytem
  i podsumować zmiany;
- każda sesja `active` jest w całości odwracalna („Revert session"),
  łącznie z importem, duplikacją i usunięciem — również po restarcie
  aplikacji, z detekcją konfliktów (afterHash) i weryfikacją wyniku
  (beforeHash); sesja `committed` jest jawnie i trwale nieodwracalna;
- wymiana profilu przy aktywnej sesji jest zablokowana; wznowienie sesji
  z niezgodnym `profileHash` wymaga jawnej decyzji;
- rozmowę można wznowić po restarcie i kontynuować z zachowanym
  kontekstem;
- błędne argumenty (zły blok, zakres, model, nieaktualna rewizja, ścieżka
  spoza approved roots) zwracają czytelny błąd/`needs_confirmation`/
  `conflict` do agenta zamiast przerywać pętlę;
- testy jednostkowe: mapowanie schematów per provider i wariant, walidacja
  argumentów, protokół WAL (w tym recovery i konflikt hashy), maszyna
  stanów sesji, SessionStore (zapis przyrostowy, wznowienie, kompaktowanie,
  wiązanie profilu), migracja indeksu biblioteki.

## 5. Ryzyka i mitygacje

| Ryzyko | Mitygacja |
| --- | --- |
| Rosnący koszt kontekstu przy długich/wznawianych rozmowach | zwięzłe wyniki narzędzi, limit tur, kompaktowanie starszych tur przy wznowieniu |
| Agent wykonuje niechciane zmiany | dziennik WAL + „Revert session"/`revert_last_agent_action`, `.destructive` i obce ścieżki za potwierdzeniem |
| Równoległe mutacje (użytkownik w GUI + sesja agenta + MCPBridge) | jeden serializowany executor + `expectedRevision` na każdej mutacji; kolizja = `conflict`, nigdy ciche nadpisanie |
| Awaria aplikacji w trakcie mutacji | protokół WAL `prepared → committed` z fsync i recovery przy starcie |
| Revert po restarcie trafia na ręczne zmiany użytkownika | para `beforeHash`/`afterHash` per wpis, częściowy revert z raportem konfliktów |
| Kasowanie stagingu unieważnia obiecany revert | staging kasowany wyłącznie w stanie `committed`; „Empty trash" z ostrzeżeniem |
| Rozjazd definicji narzędzi między czatem, mostkiem MCP i `MG101MCP` | wspólne komendy domenowe i walidacja w `MG101Tools`; dwa jawne warianty transportowe z testami kontraktowymi |
| Otwarcie endpointu MCP | tylko localhost, token per uruchomienie, faza opcjonalna |
| Różnice providerów (tool use vs function calling) | wspólna abstrakcja + testy kontraktowe obu mapowań |
| Migracja biblioteki (faza 0) na istniejących danych użytkownika | kopia zapasowa katalogu przed migracją; deduplikacja ograniczona do fabrycznych duplikatów z defektu; testy na rzeczywistym stanie PatchLibrary |
| Zmiana profilu unieważnia identyfikatory i dziennik | `profileHash` w `session.json`, blokada wymiany profilu przy sesji `active`, jawna decyzja przy wznowieniu |
| Stan aplikacji zmienia się między sesjami rozmowy | wymuszenie świeżych odczytów po wznowieniu (prompt systemowy), stabilne ID z fazy 0, `expectedRevision` jako ostateczna zapora |

## Historia

- v1 (2026-07-04): wersja pierwotna.
- v2 (2026-07-04): po pierwszej recenzji — MG101Tools, dziennik transakcji,
  polityka ścieżek, serwisy bezpanelowe, SessionStore, faza 0.
- v3 (2026-07-04): po drugiej recenzji — para `beforeHash`/`afterHash`
  i protokół WAL `prepared → committed` z recovery (pkt 1, 5); maszyna
  stanów sesji `active`/`committed`/`reverted` i cykl życia stagingu
  (pkt 2); jawne `patchID` + `expectedRevision` we wszystkich mutacjach,
  `select_patch` jako operacja czysto UI (pkt 3); usunięcie `undo`/`redo`
  z API agenta na rzecz `revert_last_agent_action`/`revert_session`
  (pkt 4); rozdzielenie komend domenowych od dwóch wariantów schematów
  transportowych (pkt 6); serializowany executor obejmujący też odczyty
  (pkt 7); doprecyzowanie fazy 0 — typ `PatchID`, bloby oryginałów,
  deduplikacja tylko fabrycznych (pkt 8); wiązanie sesji z `profileHash`
  (pkt 9); podział fazy 1 i urealnienie szacunków (pkt 10).
