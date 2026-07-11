# Narzędzia (poza buildem aplikacji)

## `extract_quicktone_images.py` — obrazy modeli z QuickTone (W4)

Wyciąga grafiki modeli (wzmacniacze/kabinety/mikrofony) z aplikacji **QuickTone**
(NUX/Cherub, JUCE) i mapuje je na modele katalogu MG-101, zapisując pliki
`<moduł>_<model_id>.png`, z których desktop ładuje obrazy w runtime.

```bash
python3 tools/extract_quicktone_images.py \
    [--app /ścieżka/QuickTone.app] \
    [--out ~/Library/Application\ Support/dev.mos.mg101studio/model-images]
```

Domyślny cel to katalog danych aplikacji (macOS). Pierwsze uruchomienie deasembluje
binarkę QuickTone (`otool -tV`, kilka minut; wynik cache'owany w `/tmp`).

### ⚠️ Własność intelektualna — NIE commituj obrazów

Grafiki QuickTone to własność **Cherub Technology Co., Ltd.** (Info.plist:
„Copyright 1998-2024 … All Rights Reserved"). QuickTone jest darmowy do użycia, ale
darmowość ≠ zgoda na redystrybucję grafik.

- **Skrypt (ten plik) trafia do repo** — to nasz kod, czyta lokalną instalację.
- **Wynik (pliki `.png`) świadomie NIE trafia do repo ani dystrybucji** — domyślnie
  ląduje w katalogu danych aplikacji, poza drzewem repo. `.gitignore` blokuje też
  ewentualny katalog `model-images/` w repo.
- Dystrybucja obrazów z aplikacją wymagałaby pisemnej zgody NUX/Cherub albo
  własnych/royalty-free renderów.
