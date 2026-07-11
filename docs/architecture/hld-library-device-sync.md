# HLD: Biblioteka, urządzenie i silnik transferu (generyczny, multi-device)

- Status: propozycja
- Data: 2026-07-11
- Autor: Maciek Ostaszewski
- Powiązane: [ADR-0002 (Rust core + Device Pack)](../adr/0002-rust-core-device-packs.md),
  [ADR-0001 (rejestr narzędzi, rewizje, WAL)](../adr/0001-agent-tool-registry.md),
  [HLD platformy SaaS](hld-saas-platform.md)
- Empiria sprzętowa: `nux/mg101-probe/docs/findings.md` (zdekodowany protokół
  MG-101: odczyt `SUB=00`, edycja live przez CC, zapis slotu `0B 01 <slot>`).

## 1. Cel

Domknięcie dwóch wymagań do przepisania aplikacji na Rust:

1. **Generyczność** — jeden silnik biblioteki i transferu działa dla dowolnego
   urządzenia; dodanie nowego multiefektu (NUX lub innego producenta) to nowy
   Device Pack z **deklaratywnym opisem topologii pamięci**, bez zmian w silniku
   ani UI.
2. **Model Biblioteka ↔ Urządzenie** — biblioteka o dowolnym rozmiarze
   (tagowanie, grupowanie) obok slotów urządzenia (User/Factory, limitowane),
   z rozróżnianiem/dopasowaniem patchy po połączeniu i transferem pojedynczym
   oraz grupowym z zachowaniem limitów.

Zasada nadrzędna (z ADR-0002): **rdzeń nie zna MG-101**. Operuje na modelu
kanonicznym i na abstrakcyjnej topologii pamięci deklarowanej przez Device Pack.

## 2. Generyczny model topologii pamięci urządzenia

Dziś (MG-101): dwa banki po 36 slotów — **User** (zapisywalny) i **Factory**
(tylko odczyt), łącznie 72. To jest szczególny przypadek ogólnego modelu.
Device Pack (`profile`, dane) deklaruje topologię, a silnik czyta ją generycznie.

```jsonc
// profile.storage — fragment profilu urządzenia (dane, nie kod)
{
  "banks": [
    { "id": "user",    "label": "User",    "slots": 36, "index_base": 0,
      "writable": true,  "reorderable": true,  "erasable": false },
    { "id": "factory", "label": "Factory", "slots": 36, "index_base": 0,
      "writable": false, "reorderable": false, "erasable": false }
  ],
  "capabilities": {
    "read_slot": true, "write_slot": true, "read_bank_bulk": true,
    "has_slot_names": true, "select_by_program_change": true,
    "live_param_edit": "cc"      // lub "sysex" | "none"
  },
  "limits": { "max_writable_total": 36, "per_bank": { "user": 36 } }
}
```

Adres slotu jest zawsze parą **`(bank_id, index)`**. Inny sprzęt może mieć
`A/B/C/D × 25`, płaską listę 128, albo pojedynczy „edit buffer” — silnik nie
musi o tym wiedzieć. Trait w `device-pack-api`:

```rust
pub struct SlotAddr { pub bank: BankId, pub index: u16 }

pub trait DeviceStorage {
    fn banks(&self) -> &[BankInfo];                 // z profilu
    fn is_writable(&self, bank: &BankId) -> bool;
    fn free_slots(&self, bank: &BankId, occupied: &SlotMap) -> Vec<SlotAddr>;
    fn limits(&self) -> &StorageLimits;
}

pub trait DeviceProtocol {                          // kod packa (WASM/native)
    fn read_slot(&self, link: &mut dyn DeviceLink, a: SlotAddr) -> Result<PatchBlob>;
    fn write_slot(&self, link: &mut dyn DeviceLink, a: SlotAddr, b: &PatchBlob) -> Result<()>;
    fn read_bank(&self, link: &mut dyn DeviceLink, bank: &BankId) -> Result<Vec<PatchBlob>>;
}
```

`DeviceLink` to transport (MIDI/SysEx) — patrz §7. Silnik biblioteki i transferu
zależy **wyłącznie** od tych traitów i od modelu kanonicznego, nigdy od MG-101.

## 3. Model Biblioteki

Biblioteka to zbiór patchy niezależny od urządzenia, o dowolnym rozmiarze.
Każdy wpis (z ADR-0002 „bajty są święte”):

```rust
pub struct LibraryPatch {
    pub id: Uuid,
    pub name: String,
    pub canonical: CanonicalPatch,     // do edycji, wyszukiwania, diff
    pub blob: PatchBlob,               // oryginał, nieznane bajty zachowane
    pub origin: PatchOrigin,           // ImportedFile | PulledFromDevice | Created | Shared
    pub device_id: DeviceId,           // dla jakiej rodziny/urządzenia
    pub firmware: Option<String>,
    pub codec_version: String,
    pub content_hash: Hash,            // fingerprint kanonicznego + blob (§4)
    pub tags: BTreeSet<TagId>,         // wiele-do-wielu, swobodne
    pub groups: BTreeSet<GroupId>,     // przynależność do kolekcji
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub revision: u64,                 // rewizje z ADR-0001 → sync lokalne↔chmura
}
```

- **Tagi** — swobodne etykiety wiele-do-wielu (`metal`, `ambient`, `live-set-A`),
  do filtrowania i wyszukiwania. Nie mają kolejności.
- **Grupy** — nazwane, **uporządkowane** kolekcje (np. „Koncert 2026”, „Presety
  basowe”). Jednostka transferu grupowego: kolejność w grupie wyznacza kolejność
  zapisu do slotów. Patch może należeć do wielu grup.

Rozróżnienie celowe: tag = klasyfikacja/filtr, grupa = zestaw do wgrania „jak
leci”. Oba są metadanymi Biblioteki, nie dotykają blobu patcha.

Przechowywanie: lokalnie SQLite (desktop) / Postgres (SaaS), blob osobno
(content-addressed). Schemat identyczny — rewizje umożliwiają sync (ADR-0002 §4).

## 4. Rozróżnianie i dopasowanie po połączeniu

Po połączeniu urządzenia mamy trzy źródła patchy: sloty **User** i **Factory**
na sprzęcie oraz **Bibliotekę**. UI = trzy zakładki (User / Factory / Library).
Kluczowe: pokazać relację między nimi, nie tylko listy.

> **Uwaga implementacyjna (E4).** Zgodnie z ADR-0002 biblioteka jest
> device-agnostyczna i **nie przechowuje** `CanonicalPatch` — trzyma wyłącznie
> `blob` (bajty święte) + hashe; model kanoniczny wylicza Device Pack na żądanie.
> Fingerprint liczony jest przez **maskowanie regionów** blobu (dane `MaskRange`
> z profilu), nie przez normalizację kanoniczną. **Kontrakt przestrzeni hashy:**
> wszystkie hashe (`content_hash`, `exact_hash`, `hash_at_transfer`) liczone są
> nad **reprezentacją rekordu urządzenia** (to, co idzie do/ze slotu), nie nad
> kontenerem pliku — inaczej patch z pliku po wgraniu byłby wiecznie
> `DeviceModified`. Import pliku najpierw wyłuskuje rekord urządzenia.

**Fingerprint.** `content_hash = H(canonical_normalized ‖ blob_significant)`.
Kanonik normalizujemy (pomijamy nazwę i pola nieistotne dźwiękowo, konfigurowalne
w profilu), żeby „ten sam dźwięk pod inną nazwą” dało ten sam hash dla dopasowania
brzmieniowego; osobno trzymamy `exact_hash` całego blobu do wykrycia „bit-identyczny”.

**Provenance (ślad pochodzenia).** Przy każdym transferze Biblioteka→slot
zapisujemy link:

```rust
pub struct DeviceSlotLink {
    pub device_serial: DeviceId,       // konkretny egzemplarz (stabilne id, np. USB serial)
    pub slot: SlotAddr,
    pub library_patch_id: Uuid,
    pub hash_at_transfer: Hash,
    pub transferred_at: Timestamp,
}
```

**Stan trójdrożny per slot** (liczony po podłączeniu, przez porównanie
bieżącego hasha slotu z linkiem i z Biblioteką):

| Stan | Znaczenie | Akcja UI |
|---|---|---|
| `InSync` | slot = powiązany patch z Biblioteki (hash zgodny) | pokaż link do wpisu |
| `DeviceModified` | slot był z Biblioteki, ale zmieniony na urządzeniu | „zaktualizuj Bibliotekę” / „przywróć” |
| `DeviceOnly` | slot nie pasuje do niczego w Bibliotece | „importuj do Biblioteki” |
| `LibraryNewer` | powiązany wpis w Bibliotece nowszy niż slot | „wyślij ponownie” |
| `Empty` | slot pusty/domyślny | cel transferu |

Factory jest zawsze read-only → dla niego tylko `DeviceOnly`/`InSync` + akcja
„importuj do Biblioteki” (bez zapisu na sprzęt).

To odpowiada wprost na Twoje pytanie „jak rozróżniać patche w bibliotece od tych
na urządzeniu”: nie po nazwie (zawodna), lecz po **fingerprincie + śladzie
pochodzenia**, z czytelnym stanem synchronizacji.

## 5. Silnik transferu

Generyczny, zależny tylko od `DeviceStorage`/`DeviceProtocol`. Operacje:

- **Push** Biblioteka → slot(y) urządzenia (pojedynczo lub grupą).
- **Pull** slot(y) urządzenia → Biblioteka (import, także całych banków).
- **Sync** wg stanu trójdrożnego (§4) z decyzją per konflikt.

**Transfer grupowy z limitami.** Wejście: grupa (uporządkowana) + bank docelowy
+ strategia rozmieszczenia (`NextFree` | `FromSlot(n)` | `Overwrite(range)`).
Silnik:

1. czyta `limits`/`writable` z profilu; Factory i przepełnienie odrzuca **przed**
   jakimkolwiek zapisem (walidacja całościowa, nie w połowie),
2. planuje mapowanie `patch[i] → SlotAddr` (raport planu do zatwierdzenia),
3. wykonuje zapisy transakcyjnie: **WAL** (ADR-0001) + `expectedRevision` na
   slocie — konflikt (ktoś/coś zmieniło slot) przerywa i pozwala cofnąć,
4. po każdym zapisie aktualizuje `DeviceSlotLink` i stan slotu.

**Bezpieczeństwo zapisu** (kontynuacja zasady z `mg101-probe`: odczyt swobodny,
zapis pod kontrolą): każdy `write_slot`/`0B 01` wymaga jawnego planu i potwierdzenia;
kodek waliduje długość/zakresy przed wysłaniem; brak „ślepego” zapisu surowych
bajtów. Dry-run pokazuje dokładny plan slotów i różnice.

**Limity są danymi profilu**, więc dla innego urządzenia (np. 128 slotów, brak
Factory) ten sam silnik działa bez zmian.

## 6. UI (powłoka desktop Slint; web JS później) — trzy zakładki

```
┌ Device: NUX MG-101 (fw 202407110808) ─────── ● połączony ─┐
│ [ User (36) ] [ Factory (36) ] [ Library (∞) ]            │
│                                                            │
│  User:                          Library (filtr: tag/grupa) │
│   01A ● InSync   „Clean DI”      ▸ Grupa „Koncert 2026” (8) │
│   01B ⚠ DeviceMod „Lead”         ▸ Grupa „Bas” (5)          │
│   02A ○ Empty                    #metal #ambient #live      │
│   …                              [ patch ] [ patch ] …      │
│                                                            │
│  → przeciągnij patch/grupę na bank  |  ← importuj slot     │
└────────────────────────────────────────────────────────────┘
```

- Zakładki **User/Factory** = widok slotów sprzętu ze stanem trójdrożnym.
- Zakładka **Library** = pełna biblioteka z filtrem po tagach/grupach, drzewem grup.
- Transfer: pojedynczy patch lub cała grupa; plan + potwierdzenie; pasek postępu.
- Desktop teraz: **Slint** (natywny Rust, `midir`). Web później: osobny frontend
  JS (WebMIDI, Chrome/Edge) nad tym samym rdzeniem. Wspólny rdzeń, nie kod UI.

## 7. Umiejscowienie kodu urządzenia (z `mg101-probe`)

Dorobek z `mg101-probe` nie jest wyrzucany — wchodzi jako fundament warstwy device:

- **`device-link` (nowy crate)** — transport MIDI/SysEx cross-platform (`midir`),
  reassembler SysEx + ramkowanie komunikatów kanałowych (już napisane i przetestowane
  w `mg101-probe`: `SysexAssembler`, `message_len`), rozróżnianie endpointów po
  stabilnym `unique_id`, WebMIDI za tym samym traitem `DeviceLink`.
- **`pack-nux-mg101/protocol`** — dialekt SysEx MG-101: ramki `F0 43 58 70 …`,
  `read_slot` (`09/0B 00 <idx>`), `write_slot` (`0B 01 <slot> <189B>`), mapa CC
  (public 0–91), Program Change. Wprost z `findings.md`.
- **`pack-nux-mg101/codec`** — transkodowanie: rekord urządzenia (189 B, same
  parametry) ↔ model kanoniczny ↔ plik `.mg101patch` (kontener 8402 B z osadzonym
  WAV IR). Round-trip testowany na istniejących plikach (Swift jako oracle).
- **CLI `mg101-probe`** zostaje jako narzędzie diagnostyczne dev (`list/monitor/
  probe/dump`), nie część produktu.

Wniosek z eksperymentu MITM (capture 08): transparentny passthrough „ta sama
nazwa” nie działa — dlatego produkt **jest hostem** (bezpośredni link do sprzętu),
a nie pośrednikiem dla QuickTone. To upraszcza architekturę.

## 8. Roadmapa przepisania (fazowa, aplikacja Swift żyje jako referencja)

Przepisanie „całości naraz” jest ryzykowne; sekwencja z bramkami:

- **Faza 0 — szkielet.** Workspace Rust wg ADR-0002 (`core`, `device-pack-api`,
  `device-link`, `pack-nux-mg101`, `desktop`, `web`, `server`). Wciągnięcie
  `SysexAssembler`/transportu z `mg101-probe` do `device-link`.
- **Faza 1 — rdzeń + pack MG-101 (offline).** Model kanoniczny, `device-pack-api`
  (w tym `DeviceStorage`), profil MG-101 (`storage` z §2), codec. **Bramka:**
  round-trip 1:1 na istniejących `.mg101patch` vs Swift.
- **Faza 2 — link do sprzętu (na żywo).** `device-link` + `DeviceProtocol` MG-101.
  Najpierw **read-only dump banku** (testowalny na Twoim urządzeniu od zaraz),
  potem zapis pod kontrolą (WAL + plan). **Bramka:** wierny zrzut 72 slotów.
- **Faza 3 — Biblioteka.** Store (SQLite), tagi, grupy, fingerprint, provenance,
  stan trójdrożny (§3–4). Headless, w pełni testowalne.
- **Faza 4 — silnik transferu.** Push/Pull/Sync, transfer grupowy, limity, WAL
  (§5). **Bramka:** wgranie grupy na User z zachowaniem limitów + cofnięcie.
- **Faza 5 — powłoka UI + agent.** Tauri + web, trzy zakładki, MCP lokalny
  (`rmcp`), integracja agentowa (ADR-0001) przeciw modelowi kanonicznemu.
- **Faza 6 — SaaS/chmura.** Sync rewizji, sharing, biblioteka centralna
  (osobny HLD platformy SaaS).

Aplikacja Swift jest wygaszana dopiero po Fazie 5 (ADR-0002 §7).

## 9. Kwestie otwarte

1. **Tożsamość egzemplarza sprzętu** (`device_serial`) — czy MG-101 udostępnia
   numer seryjny przez SysEx? Jeśli nie, provenance wiążemy per „to urządzenie na
   tym porcie” + hash zawartości banku jako słabszy identyfikator. Do zbadania
   w Fazie 2 (zapytanie tożsamości było w handshake — `findings.md`).
2. **Normalizacja kanoniczna do fingerprintu** — które pola pomijać (na pewno
   nazwa; IR? poziomy USB?) definiuje profil; ustalić empirycznie w Fazie 3.
3. **Grupy vs foldery** — czy grupy mogą być zagnieżdżone (drzewo) czy płaskie?
   Start: płaskie + tagi; zagnieżdżanie odłożone.
4. **Konflikt nazw slotów** — MG-101 ma nazwy slotów; przy imporcie do Biblioteki
   zachowujemy nazwę urządzenia jako początkową `name`.
```
