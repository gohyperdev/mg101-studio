# ADR-0001: Rejestr narzędzi aplikacji i agent loop z tool callingiem

- Status: zaproponowany
- Data: 2026-07-04 (v3 — korekty po drugiej recenzji architektonicznej)
- Decydenci: Maciek Ostaszewski
- Dokument powiązany: [HLD integracji agentowej](../architecture/hld-agent-integration.md)

## Kontekst

MG101 Studio ma dziś dwa niezależne mechanizmy „agentowe":

1. **Agent w aplikacji** (`AgentAPIClient`) w wzorcu one-shot: cały kontekst
   (aktualny patch + katalog modeli) jest wklejany do promptu, a LLM zwraca
   jednorazowo JSON z listą operacji. Brak tool callingu, brak dialogu, brak
   dostępu do biblioteki patchy; agent widzi wyłącznie zaznaczony patch
   i dysponuje sześcioma operacjami zapisu.
2. **Serwer MCP** (`MG101MCP`) działający na plikach `.mg101patch` przez
   stdio, bez żadnego połączenia ze stanem działającej aplikacji.

Cel: pełna manipulacja danymi aplikacji (biblioteka, patch, IR, import,
eksport) w formie dialogu z agentem, z zachowaniem gwarancji
bezpieczeństwa edytora (walidacja profilem, nowe pliki zamiast nadpisywania,
możliwość cofnięcia zmian) oraz z trwałością rozmów między uruchomieniami
aplikacji.

## Rozważane opcje

### A. Rozbudowa obecnego wzorca one-shot

Rozszerzenie schematu JSON o kolejne operacje i większy kontekst w prompcie.

- Zalety: najmniejszy nakład pracy.
- Wady: brak pętli odczyt → decyzja → zmiana → weryfikacja; kontekst rośnie
  liniowo z biblioteką; agent nie może niczego doczytać na żądanie; kruche
  parsowanie odpowiedzi tekstowej; brak rozmowy wieloturowej.

### B. Wewnętrzny rejestr narzędzi + natywny agent loop (WYBRANA)

Definicje narzędzi (nazwa, opis, JSON Schema, klasa operacji) w osobnym
module `MG101Tools` bez zależności od GUI; wykonania (executory) osobno dla
aplikacji (na `StudioState`) i dla plikowego MCP. Pętla tool-use na
Anthropic Messages API / OpenAI function calling. Narzędzia odczytu wykonują
się automatycznie, mutacje przechodzą przez dziennik transakcji sesji
z operacją „Revert session".

- Zalety: pełny, iteracyjny dostęp do danych; naturalna rozmowa; jedna
  definicja narzędzi dla czatu, wewnętrznego mostka MCP i plikowego
  `MG101MCP`; wykorzystuje istniejącą walidację `MG101Core`; typowane
  wywołania zamiast parsowania tekstu.
- Wady: większy nakład (moduł definicji, dziennik transakcji, pętla,
  streaming, UI czatu); koszty API rosną z liczbą tur.

### C. Aplikacja jako serwer MCP dla zewnętrznych agentów

Wystawienie tych samych narzędzi przez MCP (Streamable HTTP na localhost),
aby zewnętrzny agent (np. Claude Code) sterował żywą aplikacją.

- Zalety: dostęp z zewnątrz bez duplikowania logiki; spójność z opcją B.
- Wady: nie zastępuje czatu w aplikacji; wymaga zabezpieczenia endpointu
  (token, wyłącznie localhost); GUI nie może być serwerem stdio, więc
  konieczny transport HTTP.

### D. Osadzenie zewnętrznego agenta (np. `claude` CLI w trybie headless)

Aplikacja uruchamia zewnętrzny proces agenta wskazując MCP zwrotne do siebie.

- Zalety: gotowa pętla agentowa, zero własnego kodu pętli.
- Wady: twarda zależność od zewnętrznego narzędzia i jego licencjonowania;
  brak kontroli nad UX; nieprzewidywalne środowisko użytkownika końcowego.

## Decyzja

Wybieramy **opcję B** jako rdzeń, z **opcją C jako fazą opcjonalną** budowaną
na tych samych definicjach narzędzi. Opcja A zostaje wycofana po migracji;
opcja D odrzucona.

Kluczowe rozstrzygnięcia:

1. **Podział definicja/wykonanie** — nowy target `MG101Tools` (zależny tylko
   od `MG101Core`) zawiera schematy narzędzi i typy operacji; executor GUI
   działa na `StudioState`, executor plikowy w `MG101MCP`. Żaden kod
   współdzielony nie zależy od targetu GUI (wymóg grafu zależności
   w `Package.swift`).
2. **Dziennik transakcji sesji zamiast migawek patchy** — każda mutacja
   (w tym strukturalne: import, duplikacja, usunięcie) przechodzi protokół
   write-ahead (`prepared → mutacja atomowa → committed`, recovery przy
   starcie) i zapisuje operację odwrotną wraz z parą hashy
   `beforeHash`/`afterHash`. „Revert session" odtwarza dziennik od końca:
   konflikt wykrywa porównaniem z `afterHash`, wynik odwrotności weryfikuje
   `beforeHash`. Usunięcia są miękkie (staging); fizyczne kasowanie
   wyłącznie po nieodwracalnym „Commit session" (maszyna stanów sesji
   `active`/`committed`/`reverted`). Dziennik jest niezależny od globalnego
   undo stacku UI, który jest czyszczony przy zmianie zaznaczenia i nie
   obsłuży sesji wielopatchowej; `undo`/`redo` nie są wystawiane agentowi.
3. **Klasy narzędzi i polityka ścieżek** — narzędzia mają klasę
   `read` / `write` / `filesystem` / `destructive`. Narzędzia `filesystem`
   (import, eksport, IR z pliku WAV) przyjmują wyłącznie ścieżki z katalogów
   zatwierdzonych przez użytkownika w danej sesji; `destructive` zawsze
   wymaga potwierdzenia w UI.
4. **Serwisy bezpanelowe** — logika importu/eksportu/IR zostaje wydzielona
   do serwisów przyjmujących URL; `NSOpenPanel`/`NSSavePanel` pozostają
   wyłącznie w warstwie GUI. Handlery narzędzi nigdy nie otwierają dialogów.
5. **Współbieżność** — pętla agentowa (sieć, streaming, parsowanie) działa
   poza MainActor; wszystkie operacje narzędziowe na `StudioState` —
   odczyty i mutacje — przechodzą przez jeden serializowany executor na
   MainActor (odczyty zwracają niemutowalne migawki). Mutacje przyjmują
   `patchID` + `expectedRevision` (optymistyczna kontrola współbieżności);
   globalne zaznaczenie UI nie jest warunkiem poprawności żadnej mutacji.
6. **Trwałość rozmów** — sesje agenta (wiadomości, wywołania narzędzi,
   dziennik transakcji) są utrwalane w Application Support i możliwe do
   wznowienia po restarcie aplikacji.
7. **Walidacja pozostaje w `MG101Core`** — każdy argument narzędzia jest
   weryfikowany profilem urządzenia i katalogiem modeli, jak dotychczas.
8. **Plikowy `MG101MCP` pozostaje** do pracy wsadowej/offline i przechodzi
   na definicje z `MG101Tools`.

## Warunek wstępny (faza 0)

Integracja wymaga stabilnych identyfikatorów i trwałych metadanych
biblioteki, których dziś brak — to istniejący defekt wersji 1.2, nie tylko
blocker agenta:

- identyfikatory patchy fabrycznych są losowane przy każdym uruchomieniu
  (`PatchLibraryStore.load`), więc odwołania agenta nie przetrwają restartu;
- `origin`, `sourceName` i bazowy `original` nie są utrwalane — po
  restarcie edytowany patch fabryczny wraca jako „Imported", znika baza
  diffa, a kolejne edycje tworzą zduplikowane pliki pod nowymi UUID
  (potwierdzone: trzy identyczne kopie tego samego patcha w PatchLibrary).

Naprawa (indeks metadanych biblioteki, typ `PatchID` jako String
z deterministycznymi ID fabrycznych `factory-01…36`, licznik `revision`,
utrwalenie oryginałów jako niemutowalne bloby) jest wydzielona jako faza 0
i poprzedza implementację agenta.

## Konsekwencje

- Powstają: target `MG101Tools`, dziennik transakcji, serwisy bezpanelowe,
  pętla agentowa, magazyn sesji rozmów i UI czatu (szczegóły w HLD).
- `LocalAgentPlanner` zostaje jako fallback offline bez zmian kontraktu.
- Sekrety pozostają w Keychain (`KeychainStore`), konfiguracja providerów
  w `AISettingsView` bez zmian koncepcyjnych.
- Wzrost kosztów wywołań API proporcjonalny do liczby tur; mitygacja przez
  zwięzłe wyniki narzędzi, limit iteracji i kompaktowanie historii przy
  wznowieniu sesji.

## Historia

- v1 (2026-07-04): wersja pierwotna — wspólny rejestr w targecie GUI,
  checkpoint oparty na migawkach patchy.
- v2 (2026-07-04): korekty po recenzji — wydzielenie `MG101Tools`, dziennik
  transakcji zamiast migawek, klasa `filesystem` z polityką ścieżek, serwisy
  bezpanelowe, pętla poza MainActor, trwałość sesji rozmów, faza 0
  (stabilne ID i metadane biblioteki); HLD przeniesiony do
  `docs/architecture/`.
- v3 (2026-07-04): korekty po drugiej recenzji — protokół WAL z parą hashy
  `beforeHash`/`afterHash` i recovery, maszyna stanów sesji
  (`active`/`committed`/`reverted`) ze stagingiem kasowanym dopiero po
  „Commit session", `patchID` + `expectedRevision` we wszystkich mutacjach,
  usunięcie `undo`/`redo` z API agenta, serializowany executor obejmujący
  odczyty, doprecyzowanie fazy 0 (typ `PatchID`, bloby oryginałów,
  `revision`); szczegóły w HLD v3.
