//! Katalog wzorców perkusji (DRUM) MG-101 i kodowanie sterowania na żywo.
//!
//! Wzorzec wybiera się dwupoziomowo: **grupa** (ROCK, CTRY, …) → **wzorzec**
//! wewnątrz grupy. Urządzenie adresuje wzorzec pojedynczą wartością CC82 —
//! płaskim indeksem liczonym kumulatywnie przez wszystkie grupy (zmiana grupy
//! skacze na pierwszy wzorzec grupy). Katalog i indeksy są tu jednym źródłem
//! prawdy dla UI (dwa ComboBoxy) oraz narzędzi agenta.
//!
//! Mapa CC/SysEx zmierzona monitorem MIDI (2026-07):
//! - CC81 = Play/Stop (1/0), CC82 = wzorzec (indeks płaski), CC83 = Volume 0–100.
//! - Tempo (odczyt i zapis) = SysEx `F0 43 58 70 7E 02 19 03 32 32 32 <hi> <lo> 00 F7`,
//!   gdzie BPM = hi*128 + lo.

/// Numery Control Change sterowania DRUM (mapa publiczna QuickTone, potwierdzona).
pub const CC_DRUM_TRANSPORT: u8 = 81;
pub const CC_DRUM_PATTERN: u8 = 82;
pub const CC_DRUM_VOLUME: u8 = 83;

/// Zakres tempa DRUM (BPM). Urządzenie akceptuje typowo 40–240.
pub const BPM_MIN: u16 = 40;
pub const BPM_MAX: u16 = 240;

/// Grupa wzorców: krótka etykieta jak na urządzeniu + lista wzorców po kolei.
pub struct DrumGroup {
    pub name: &'static str,
    pub patterns: &'static [&'static str],
}

/// Pełny katalog (odczytany z QuickTone). Kolejność grup i wzorców = kolejność
/// indeksowania CC82 (patrz [`pattern_cc`]).
pub const GROUPS: &[DrumGroup] = &[
    DrumGroup {
        name: "ROCK",
        patterns: &[
            "Standard",
            "Swing Rock",
            "Power Beat",
            "Smooth",
            "Mega Drive",
            "Hard Rock",
            "Boogie",
        ],
    },
    DrumGroup {
        name: "CTRY",
        patterns: &[
            "Walk Line",
            "Blue Grass",
            "Country",
            "Waltz",
            "Train",
            "Ctry Rock",
            "Slowly",
        ],
    },
    DrumGroup {
        name: "BLUES",
        patterns: &[
            "Slow Blues",
            "Chicago",
            "R&B",
            "Blues Rock",
            "Road Train",
            "Shuffle",
        ],
    },
    DrumGroup {
        name: "METAL",
        patterns: &[
            "2X Bass",
            "Close Beat",
            "Heavy Bass",
            "Fast",
            "Holy Case",
            "Open Hat",
            "Epic",
        ],
    },
    DrumGroup {
        name: "FUNK",
        patterns: &[
            "Bouns",
            "East Coast",
            "New Mann",
            "R&B Funk",
            "80 Funk",
            "Soul",
            "Uncle Jam",
        ],
    },
    DrumGroup {
        name: "MET",
        patterns: &[
            "4/4 4th",
            "4/4 8th",
            "4/4 16th",
            "4/4 2nd Tri",
            "4/4 4th Tri",
            "4/4 8th Tri",
            "3/4 4th",
            "3/4 8th",
        ],
    },
    DrumGroup {
        name: "BALD",
        patterns: &[
            "Bluesy",
            "Grooves",
            "Bald Rock",
            "Slow Rock",
            "Tutorial",
            "R&B Bald",
            "Gospel",
        ],
    },
    DrumGroup {
        name: "POP",
        patterns: &[
            "Beach Side",
            "Big City",
            "Funky Pop",
            "Modern",
            "School Pop",
            "Motown",
            "Resistor",
        ],
    },
    DrumGroup {
        name: "REGGAE",
        patterns: &[
            "Sherriff",
            "Santeria",
            "Reggae 3",
            "Reggae 4",
            "Reggae 5",
            "Reggae 6",
            "Reggae 7",
        ],
    },
    DrumGroup {
        name: "ELEC",
        patterns: &["Elec 1", "Elec 2", "Elec 3", "ELEC-EDM", "ELEC-TECH"],
    },
];

/// Kumulatywny indeks pierwszego wzorca grupy (baza CC82 dla grupy).
pub fn group_base(group_idx: usize) -> u8 {
    GROUPS
        .iter()
        .take(group_idx)
        .map(|g| g.patterns.len())
        .sum::<usize>() as u8
}

/// Wartość CC82 dla (grupa, wzorzec). `None`, gdy indeksy poza zakresem.
pub fn pattern_cc(group_idx: usize, pattern_idx: usize) -> Option<u8> {
    let g = GROUPS.get(group_idx)?;
    if pattern_idx >= g.patterns.len() {
        return None;
    }
    Some(group_base(group_idx) + pattern_idx as u8)
}

/// Odwrotność: z płaskiej wartości CC82 → (indeks grupy, indeks wzorca).
pub fn locate(cc: u8) -> Option<(usize, usize)> {
    let mut acc = 0usize;
    for (gi, g) in GROUPS.iter().enumerate() {
        if (cc as usize) < acc + g.patterns.len() {
            return Some((gi, cc as usize - acc));
        }
        acc += g.patterns.len();
    }
    None
}

/// Czytelna etykieta wzorca dla danej wartości CC82 (np. "CTRY / 01 Walk Line").
pub fn pattern_label(cc: u8) -> Option<String> {
    let (gi, pi) = locate(cc)?;
    let g = &GROUPS[gi];
    Some(format!("{} / {:02} {}", g.name, pi + 1, g.patterns[pi]))
}

/// Wyszukuje (grupa, wzorzec) po nazwie grupy (bez rozróżniania wielkości liter)
/// oraz wzorca podanego jako 1-based numer LUB nazwa. Dla narzędzi agenta.
pub fn find(group: &str, pattern: &str) -> Option<(usize, usize)> {
    let gi = GROUPS
        .iter()
        .position(|g| g.name.eq_ignore_ascii_case(group.trim()))?;
    let g = &GROUPS[gi];
    let pat = pattern.trim();
    // Numer 1-based?
    if let Ok(n) = pat.parse::<usize>() {
        if n >= 1 && n <= g.patterns.len() {
            return Some((gi, n - 1));
        }
    }
    // Nazwa (dopuszcza wiodące "NN ").
    let needle = pat.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ');
    let pi = g
        .patterns
        .iter()
        .position(|p| p.eq_ignore_ascii_case(needle) || p.eq_ignore_ascii_case(pat))?;
    Some((gi, pi))
}

/// Buduje ramkę SysEx ustawiającą tempo DRUM (ta sama ramka, którą urządzenie
/// wysyła przy zmianie tempa — replay jest bezpieczny). BPM przycięte do zakresu.
pub fn tempo_sysex(bpm: u16) -> Vec<u8> {
    let clamped = bpm.clamp(BPM_MIN, BPM_MAX);
    let hi = (clamped / 128) as u8;
    let lo = (clamped % 128) as u8;
    vec![
        0xF0, 0x43, 0x58, 0x70, 0x7E, 0x02, 0x19, 0x03, 0x32, 0x32, 0x32, hi, lo, 0x00, 0xF7,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_ten_groups_and_expected_total() {
        assert_eq!(GROUPS.len(), 10);
        let total: usize = GROUPS.iter().map(|g| g.patterns.len()).sum();
        assert_eq!(total, 68);
    }

    #[test]
    fn cc82_is_cumulative_and_zero_based() {
        // ROCK 01 → 0
        assert_eq!(pattern_cc(0, 0), Some(0));
        // CTRY 01 → 7 (po 7 wzorcach ROCK)
        assert_eq!(pattern_cc(1, 0), Some(7));
        // ELEC 05 → 67 (ostatni)
        let elec = GROUPS.len() - 1;
        assert_eq!(pattern_cc(elec, 4), Some(67));
        // poza zakresem
        assert_eq!(pattern_cc(0, 99), None);
    }

    #[test]
    fn locate_inverts_pattern_cc() {
        for cc in 0u8..68 {
            let (gi, pi) = locate(cc).unwrap();
            assert_eq!(pattern_cc(gi, pi), Some(cc));
        }
        assert_eq!(locate(68), None);
    }

    #[test]
    fn find_by_number_and_name() {
        assert_eq!(find("ctry", "1"), Some((1, 0)));
        assert_eq!(find("CTRY", "Walk Line"), Some((1, 0)));
        assert_eq!(find("CTRY", "01 Walk Line"), Some((1, 0)));
        assert_eq!(find("ELEC", "ELEC-EDM"), Some((9, 3)));
        assert_eq!(find("nope", "1"), None);
    }

    #[test]
    fn tempo_sysex_encodes_bpm() {
        // 120 → hi=0 lo=0x78
        let f = tempo_sysex(120);
        assert_eq!(f[11], 0x00);
        assert_eq!(f[12], 0x78);
        assert_eq!(f.first(), Some(&0xF0));
        assert_eq!(f.last(), Some(&0xF7));
        // 160 → hi=1 lo=32
        let f = tempo_sysex(160);
        assert_eq!(f[11], 1);
        assert_eq!(f[12], 32);
        // clamp
        assert_eq!(tempo_sysex(9999)[12] as u16 + tempo_sysex(9999)[11] as u16 * 128, BPM_MAX);
    }
}
