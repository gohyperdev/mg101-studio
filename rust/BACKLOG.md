# Backlog techniczny (uwagi z review, per epik)

## Z review E0 (in-transcript, Fable 5 — naprawione lub zaplanowane)
- [x] K1: CI supply-chain — `permissions: contents: read` + pinowana akcja
  `bytecodealliance/actions/wasmtime/setup@v1` zamiast `curl | bash`
  niewersjonowanego skryptu.
- [x] K2: `SysexAssembler` — System Common (0xF1..=0xF7) KASUJE running status
  (było: ustawiał `running_status=b`, więc luźny `F7` rodził śmieciowe ramki
  `[F7,x]`); osierocony `F7` ignorowany; `>=` → `==` przy zamknięciu ramki.
  +2 testy regresyjne.
- N (niekryt.): bufor SysEx bez górnego limitu (cap ~64 KiB); nieograniczony
  `mpsc` w MidirLink; `[workspace.dependencies]` puste mimo duplikacji serde/
  sha2; `device-pack-api` ciągnie `midir` tranzytywnie (feature `hardware`);
  brak MSRV `rust-version`; cache CI; `MockLink` za featurą `test-util`.

## Z review E1 (in-transcript, Fable 5 — APPROVE, brak uwag krytycznych)
- Kodek bezstratny potwierdzony konstrukcyjnie (blob = źródło prawdy) + test
  adwersaryjny + bramka 36/36 + dowód WASM. Niekryt.: mutacja `name`/`bpm`/`ir_*`
  na CanonicalPatch ignorowana przy encode (akcesory zamiast pól pub); guard
  `record_size` w `encode` (panic `copy_from_slice` przy złym profilu);
  `debug_assert` długości w `zip`; MSRV dla `is_multiple_of`; nazwa `wrong_len`.

## Z review E2 (in-transcript, Fable 5 — naprawione lub zaplanowane)
- [x] K1: `device_index` odrzuca indeks poza bankiem (>= 36) — było: `user/40`
  po cichu adresowało slot Factory, a `index >= 128` wstawiał bajt >= 0x80 do
  SysEx (niepoprawna ramka). Granica banku gwarantuje też 7-bitowość idx. +test.
- [x] K2: `write_slot` wymusza `blob.len() == 189` (SLOT_RECORD_LEN) — było:
  tylko niepustość+7bit; zapis złej długości fire-and-forget mógł uszkodzić
  rekord slotu. +test. Zaktualizowano komentarze prowizoryczności (mapowanie
  banków potwierdzone sprzętowo w findings.md).
- N (niekryt.): dump.rs — nagłówek bloku bez bank/idx (przesunięcie przy
  częściowym zrzucie), `exit(3)` przy `ok<72`, `u16::try_from` na długości;
  guard `write_slot` na bank `factory` (ReadOnly); walidacja długości w
  `read_slot`; drenaż `poll()` przed żądaniem; wypis nazwy portu w dump.


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
- [x] Duplikacja `resources/*.json` vs `archive/swift-v1/Sources/MG101Core/
  Resources/` — po E9 (archiwizacja Swift v1) ryzyko dryfu zamknięte: archiwum
  jest zamrożone, źródłem prawdy jest `rust/crates/pack-nux-mg101/resources/`.

## Do E2 (przed pracą na żywym sprzęcie) — ZROBIONE
- [x] `SysexAssembler`: realtime passthrough, przerwanie uciętego SysEx (+testy).
- [x] `device-pack-api`: `per_bank`, `erasable`, `read_bank`.
- [x] `read_slot`: pętla z deadlinem (nie martwa na sprzęcie).

## Integracja sprzętowa E2 (wymaga fizycznego MG-101 + zgody)
- Potwierdzić mapowanie `bank→indeks` (`user`=0.., `factory`=36..) na sprzęcie.
- `write_slot` fire-and-forget → dodać oczekiwanie na ACK zapisu (capture 04b
  pokazuje, że sprzęt ACK-uje) [S4 z review E2].
- Obsługa części `09` (54 B) presetu obok `0B` (189 B) — pełny preset to para.
- Transkodowanie rekord urządzenia (189 B) ↔ plik `.mg101patch` (8402 B, IR) —
  osobny kodek (E1 obsługuje plik, E2 rekord wire).

## Nity z review E2 (opcjonalne)
- `SysexAssembler`: SysEx/system-common powinny kasować running status; luźny
  `F7` bez `F0` ustawia `running_status=F7` (tani guard).
- `ProtocolError`: wariant `Link(...)` zamiast mapowania błędów transportu na
  `BadResponse`; wariant `ReadOnly` obecnie martwy.

## Z review E3 (naprawione lub zaplanowane)
- [x] `plan_recovery`: sentinel `""` przy braku pliku (create/delete) — było
  Rollback wskrzeszający patch; +testy (KRYTYCZNE #1).
- [x] `FileJournal::rewrite`: atomowy temp+fsync+rename — było `fs::write`
  obcinające plik przy awarii (KRYTYCZNE #2); +test.
- [x] `plan_recovery`: sort malejąco po `sequence` (kolejność inwersów).
- [x] `int_array_arg`: fallback f64→i64 (LLM emituje `[50.0]`) — parytet z v1.
- [x] `set_ir`: opis wariantu bibliotecznego z frazą o zatwierdzonym katalogu.
- Format dziennika na dysku jest v2-native (snake_case, `timestamp_ms` epoch,
  blob jako tablica). NIE jest wstecznie zgodny z JSONL v1 (`timestamp` Double
  epoka-2001, camelCase, base64). Decyzja: dziennik WAL jest efemeryczny
  (sesyjny stan crash-recovery), upgrade in-place startuje czysty dziennik —
  brak migracji zawieszonej transakcji v1. [x] POTWIERDZONE przy E9 —
  udokumentowane w `archive/swift-v1/README.md` (brak migracji WAL v1→v2).
- `session_inverses` cofa WSZYSTKIE zawieszone wpisy; v1 tylko ostatnią linię.
  Świadome odstępstwo — potwierdzić przy porcie StudioState (E4).
- Duplikacja `Kind`/nazw między `Command` a `ToolDefinition` — dodać test
  odwrotny (każda komenda ma definicję) i test spójności `kind` (E4).
- Komunikaty błędów aliasów (`bool_value` vs `bypassed`, `keys[0]` w
  `alias_string`) różnią się od v1 — tylko treść błędu, nie zachowanie.
- `sha256_hex`: `format!` per bajt — mikro-optymalizacja (dowolny moment).

## Z review E4 (naprawione lub zaplanowane)
- [x] W1: kontrakt przestrzeni hashy (fingerprint nad rekordem urządzenia, nie
  kontenerem pliku) — docstring `fingerprint.rs` + HLD §4. Egzekwować w E5.
- [x] W2: `reorder_group` odrzuca nie-permutacje (multizbiór) — było: duplikat
  `["a","a"]` gubił członka. Fix + test w obu store'ach.
- [x] W3: `record_link` → `Result` (ciche gubienie provenance fałszowało sync).
- [x] W4: warianty `LibraryError::Backend` / `InvalidInput` (koniec worka NotFound).
- [x] W5: operacje wielokrokowe SQLite (remove/delete_group/add/remove_from_group)
  w transakcjach — atomowa spójność grupa↔zwierciadło.
- [x] W6: test „oba zmienione" w silniku sync (→ DeviceModified, świadomie).
- [x] W7: wspólny `contract_suite` uruchamiany na MemoryStore i SqliteStore.
- W3-reszta: metody odczytu store (`get`/`all`/`group`/`by_tag`/`links_*`) nadal
  zwracają `Option`/`Vec` i połykają błędy backendu → `.ok()`/`filter_map`. Do
  konwersji na `Result` na starcie E5 (transfer polega na wiarygodnym odczycie).
- W6-reszta: rozważyć stan `Conflict` (slot i lib rozeszły się) — decyzja per
  konflikt w E5 (HLD §5).
- Drobne: newtype `ContentHash`/`ExactHash` zamiast aliasu `String` (silne
  typowanie); `SlotView.writable` nieużywane (egzekwować regułę Factory albo
  usunąć); `LibraryIndex.revision` martwe; blob w JSON (~4× narzut) vs
  content-addressed store (HLD §3) — na desktop OK; indeks per-tag w SQLite.

## Z review E5 (naprawione lub zaplanowane)
- [x] K1: rollback partii na WSZYSTKICH ścieżkach błędu (append prepared/committed,
  record_link, brak patcha) — nie tylko protokół/konflikt. +3 testy.
- [x] K2: wykonywalne nadpisanie konfliktu — `PlannedWrite.expected_before_hash`
  egzekwowany per slot; potwierdzenie = ustawienie bieżącego hasha. Domyka W2
  (konflikt wykrywany dla każdego slotu, nie tylko powiązanego). +test.
- [x] W1: arytmetyka indeksów w u32 (bez paniki/wrap u16); +testy index_base=1,
  FromSlot(u16::MAX), realny limit total (128 slotów/limit 100).
- [x] Drobne: unifikacja przestrzeni hashy (exec używa bibliotecznego exact_hash);
  Conflict.applied→rolled_back.
- W3 (ŚWIADOMIE ODŁOŻONE): dziennik WAL device-transferów współdzieli przestrzeń
  id z patchami (`slot:<bank>:<idx>`). Crash-recovery slotów NIE jest obsłużone —
  `plan_recovery` (biblioteczne) nie umie pisać na urządzenie. Decyzja: przy
  porcie egzekutora recovery (E6+/StudioState) rozdzielić dziennik transferów od
  bibliotecznego (osobny store + device-aware applier). Do tego czasu rollback
  partii pokrywa awarie bez crasha. Rollback nie jest dziennikowany → wpisy
  Committed cofniętych zapisów zostają (nie wykonywać `session_inverses` na tym
  dzienniku bez rozdzielenia).
- W4 (przeniesione): odczyty store `Option`→`Result` (awaria SQLite w push myli
  się z PatchMissing). Do zrobienia razem z rozdzieleniem dziennika w E6.
- Pull całych banków (HLD §5 „także całych banków") — dodać `pull_bank` obok
  `pull_slot` (jest `read_bank` w device-pack-api). E6/E7.
- `pull_slot`: 9 argumentów → struct parametrów. `TransferPlan` pola pub →
  re-walidacja w execute_push lub konstruktor zamknięty.

## Z review E6 (naprawione lub zaplanowane)
- [x] K1: sandbox ścieżek — normalizacja leksykalna (`..`/względne/pusty korzeń
  odrzucane); regresja traversal vs v1 domknięta. +testy.
- [x] K2: `mutate` sprawdza rewizję PRZED wpisem WAL (koniec osieroconych
  `prepared` przy konflikcie, które psuły revert_session). +test.
- [x] W4: `fix_selection()` po delete/revert (brak martwego selectedPatchID).
- [x] W2: usunięto martwe `approved_roots`/`approve_root` ze Studio.
- W1: `apply_inverse` podbija rewizję zamiast przywracać `revision_before` (v1) —
  świadome (bezpieczniejsze dla współbieżności); udokumentować + test w E7.
- W3: `get_diff` względem zerowego baseline (brak `original` w Bibliotece);
  `list_patches` bez `isModified`. Dodać baseline (pole/hash oryginału) w E7.
- W5: `compact_history` nie wpięte w `run()` — wpiąć przy transporcie HTTP (E6.3)
  z progiem tokenów z konfiguracji.
- W6: rozszerzenie `.mg101patch` zaszyte w studio — przenieść do profilu/packa
  (pole `file_extension`), by studio było w pełni device-agnostyczne.
- W7/W8: sanityzacja nazwy pliku w export (ucieczka `/`); potwierdzić odstępstwo
  session_inverses (wszystkie dangling vs ostatnia linia v1) + port recovery
  slotów; obie z E5-W3.

## Z review E6.3 (MCP rmcp — naprawione lub zaplanowane)
- [x] K1: `write_record` atomowy + bez nadpisania (temp+`sync_all`+`hard_link`,
  który atomowo zawodzi gdy cel istnieje) — było `exists()`+`fs::write` z
  wyścigiem TOCTOU (ciche nadpisanie cudzego patcha) i obcięciem pliku przy
  awarii. Parytet `PatchFileWriter.writeNew`. +test (brak wycieku tmp, input
  nietknięty, odrzucony zapis nie zmienia istniejącego output).
- [x] N3: `idempotent_hint: Some(false)` w anotacjach MCP (parytet v1).
- N1: dowolne ścieżki input/output w trybie plikowym — zamierzone (parytet v1,
  serwer stdio lokalny za zgodą klienta). Rozważyć opcjonalny root-sandbox
  (env/flaga) + doprecyzować model zaufania w doc/instructions. `open_world_hint`
  dla mutacji jest `false` (bo `Kind::Write`) mimo dowolnego FS — wierny v1, ale
  semantycznie mylący; przemyśleć z sandboxem.
- N2: nieznane narzędzie/zły argument → błąd protokołu `invalid_params` (v1
  zwracał tool-error `isError=true`, pozwalając LLM się poprawić). Świadome,
  zgodniejsze ze spec MCP; rozważyć `BadArgument` jako tool-error. Odnotować w ADR.
- N4: `inspect` zwarty JSON (v1 prettyPrinted+sortedKeys). Klucze sortowane
  (BTreeMap), treść równoważna; `to_string_pretty` jeśli snapshot-parytet.
- N5: `ExecError::Unsupported` jako worek na I/O — dodać wariant `Io`/`Exists`
  (lepsze komunikaty, dopasowanie w testach zamiast po treści stringa).
- N6: `is_mcp_tool` to drugie źródło prawdy obok rejestru E3 — nowe narzędzie
  plikowe nie trafi do MCP bez edycji listy. Dodać znacznik „file-capable" w
  `ToolDefinition`/`Kind` albo test krzyżowy rejestr↔filtr.
- N7: `dispatch` (sync FS) w `async call_tool` blokuje wątek executor-a przy
  równoległych żądaniach — `tokio::task::spawn_blocking` (profil/katalog w `Arc`).
- N8: testy — brak dla `set_ir`/`clear_ir` na ścieżce plikowej (WAV); `mcp`
  `tmpdir()` bez tagu per test (dziś jeden konsument, mina na przyszłość).

## Z review E7 (UI desktop — naprawione lub zaplanowane)
- [x] K1: optimistic concurrency naprawiony — mutacje VM przyjmują jawnie
  `expected_revision` (rewizja WIDZIANA przez użytkownika), zamiast doczytywać ją
  tuż przed zapisem (TOCTOU maskujący konflikt). Callbacki Slint niosą
  `detail.revision`. +test `stale_revision_is_rejected_as_conflict`.
- [x] K2: revert działa — Studio w binarce dostaje `SessionJournal` (WAL w pamięci
  na czas sesji); było: `Studio::new` bez dziennika → revert zawsze błąd.
- [x] K3: biblioteka zasilana — wpięty Import (`rfd` natywne okno → ImportPatch);
  było: pusty `MemoryStore` bez ścieżki zasilenia. +test `import_adds_patches`.
- [x] i18n: `slot.empty`/`editor.rev` zamiast twardych literałów w app.slint;
  `t-binary` zamiast „hex:".
- N (niekryt.): brak persystencji wyboru języka (zawsze EN po restarcie) —
  zapisać `Lang::code()` w ustawieniach. `MemoryStore` ulotny → rozważyć
  `SqliteStore` z pliku (persystencja biblioteki). Parytet UX (świadomie
  etapowe): zmiana modelu bloku w UI (VM ma set_model), Export/Transfer push/pull,
  panel ustawień AI, agent-chat + widok sesji (zakładki Agent/MCP/Binary to
  placeholdery; rdzeń agent-core/mcp gotowy). Inspektor Changes względem zera, nie
  oryginału (wspólne z E6/W3 baseline). `tr` → echo klucza zamiast „??". Emoji w
  UI (🔒/✕) — ryzyko tofu na Windows/FemtoVG.

## Z review E7-parytet (agent-chat/wątek — naprawione lub zaplanowane)
- [x] K1: panika BorrowMutError — import/eksport (modalny dialog rfd kręci
  zagnieżdżoną pętlę zdarzeń, w której tick pompy agenta robił drugie
  borrow_mut) — ścieżka z dialogu pobierana PRZED pożyczeniem VM (poza `wire!`).
- [x] HTTP timeout: jawny 300 s request + 15 s connect (było domyślne 30 s
  reqwest → zrywało dłuższe odpowiedzi LLM bez streamingu).
- [x] Pompa odświeża UI tylko po realnej pracy (did_work/done) — nie 25×/s.
- [x] compact_history wpięte w ścieżkę desktop (chat_history kompaktuje przy
  >100k tok) — domyka dług v1 #2 i E6/W5.
- N (niekryt.): MAX_ITERATIONS ciche wyczerpanie (znacznik + komunikat); usage
  gubione w gałęzi Err(AgentError) (zaniża koszt); persystencja ustawień AI/
  klucza (Keychain jak v1); `AutoAuthorizer` + puste approved_roots auto-
  zatwierdza pierwszą ścieżkę (zasilać roots z sesji); literał „[N wyników]" poza
  i18n; binding cfg-endpoint zrywa się po edycji.

## Nice-to-have (dowolny moment)
- CI: cache `Swatinem/rust-cache` + `concurrency` group.
- `Cargo.toml`: usunąć redundantne `[lib] name/path`; zweryfikować URL repo.
- E6: zdecydować, czy `mcp` to osobna binarka czy embed w desktop.

## W3 — parytet katalogu parametrów (follow-up)
- [x] **Transformacja wartości do jednostek fizycznych** — ZROBIONE: `Parameter::display_value`
  liczy dB (`raw/100*24-12`) oraz low/high-cut Hz (`20*50^(raw/100)`, `5000*4^(raw/100)`)
  wg `cabinet_display_tables`; edytor pokazuje `+3.6 dB`/`245 Hz` zamiast surowej 0..100.
  +test core. Uwaga historyczna poniżej:
- **Transformacja wartości do jednostek fizycznych** — katalog niesie `display_transform`
  (81 param.: liniowe `raw/100*24-12` oraz tabele `quicktone_low_cut_table[raw]` /
  `high_cut` / `cabinet_display_tables`). UI pokazuje wartość SUROWĄ 0..100, a jednostkę
  (dB/Hz) tylko w podpowiedzi zakresu — bo przeliczenia jeszcze nie ma. Do zrobienia:
  ewaluator wzorów + osadzenie tabel `quicktone_*`, rendering wartości ludzkiej
  (np. −12..+12 dB, 20..1000 Hz) obok kontrolki. Źródło: QuickTone binarka.
- **106 parametrów „inferred"** (cab 81, eq 18) — oznaczone `≈` w UI; docelowo potwierdzić
  kontrolowanym eksportem z QuickTone (evidence_policy: confirmed).
- **Etykiety wartości enum/list** — brak w katalogu (`value_labels`=0); modele mają UET
  ciągłe (2/3) lub toggle (7); brak realnych list wartości do nazwania.

## W7 — czat agenta (follow-up)
- **Auto-scroll na dół** przy nowej wiadomości — wymaga sterowania `viewport-y`
  ScrollView z Rust (po refresh). Teraz scroll działa (viewport-width + szerokość
  VerticalLayout), ale nie dojeżdża sam na dół. Do dodania: Timer/prop w oknie.

## W2 — import z urządzenia: dekoder wire→kanoniczny — ZŁAMANY
- [x] Kodek wire (ramka `0B`, 189 B) → plik 8402 B WYPROWADZONY i zweryfikowany:
  189 B = 63 ramki × 3 B, 2 wartości/ramkę (bit-packing). `pack::wire::decode_slot`
  / `decode_name`. Trafność: selektory 396/396, BPM/nazwy 36/36, parametry 99.2%
  (na sparowanych danych Factory==oracle). UI: nazwy slotów + „Import → Biblioteka".
- [ ] **Pozostała luka (0.8%): 4 parametry o wartości pliku >127** (dly time 0x45,
  cab 0x50, amp 0x21, mod 0x3D). Kanał wire jest 7-bit + 1 bit przepełnienia z b0,
  niespójnie stosowany dla pól extended-range. Do domknięcia: sparowane capture'y
  pojedynczych zmian tych parametrów z QuickTone (omiatające pełen zakres) →
  ustalić bit MSB/skalę. Dane fabryczne mają dla tych pól jedną wartość → wzór
  nie jest jednoznacznie wyprowadzalny z obecnego zbioru.
- [ ] Pola stałe w Factory (send, return, patch.max/pos, pedal; bypass amp/cab/sr;
  wah model/bypass) zlokalizowane tylko pozycyjnie przez stałe — potwierdzić na
  parach z banku User (gdzie się zmieniają).
- [ ] Region IR (0x82..) nie jest niesiony przez wire `0B` — import daje patch bez
  IR (zerowy). Preset na urządzeniu wiąże IR osobno (ramki `6C`, sloty IR) — do
  ewentualnego dołączenia przy pełnym transferze dwukierunkowym.
