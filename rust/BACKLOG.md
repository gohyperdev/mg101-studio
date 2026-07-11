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
  brak migracji zawieszonej transakcji v1. Potwierdzić przy retire v1 (E9).
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

## Nice-to-have (dowolny moment)
- CI: cache `Swatinem/rust-cache` + `concurrency` group.
- `Cargo.toml`: usunąć redundantne `[lib] name/path`; zweryfikować URL repo.
- E6: zdecydować, czy `mcp` to osobna binarka czy embed w desktop.
