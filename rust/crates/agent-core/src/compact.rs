//! Auto-kompakcja historii konwersacji (dług v1 — dowód #8). Gdy historia
//! przekracza budżet, starsze tury zwijamy do jednej syntetycznej notatki,
//! zachowując: pierwszą wiadomość użytkownika (kontekst zadania) i ogon ostatnich
//! wiadomości. **Czysta** i deterministyczna — bez modelu, bez we/wy.
//!
//! Krytyczne: nie wolno rozbić pary tool_use ↔ tool_result (inaczej API odrzuci
//! żądanie). Dlatego granicę kompakcji przesuwamy tak, by ogon zaczynał się od
//! wiadomości użytkownika/asystenta bez zawieszonych wyników narzędzi.

use crate::types::{ChatMessage, Role};

/// Szacunkowa liczba tokenów wiadomości (≈ 1 token / 4 znaki — jak w praktyce
/// dla oszczędnego przybliżenia; dokładne liczenie robi API w [`crate::Usage`]).
pub fn estimate_tokens(msg: &ChatMessage) -> usize {
    let mut chars = msg.content.len();
    for c in &msg.tool_calls {
        chars += c.name.len()
            + serde_json::to_string(&c.arguments)
                .unwrap_or_default()
                .len();
    }
    for r in &msg.tool_results {
        chars += r.content.len();
    }
    chars.div_ceil(4)
}

/// Zwija historię, jeśli szacowany rozmiar przekracza `max_tokens`. Zachowuje
/// pierwszą wiadomość użytkownika oraz jak najdłuższy ogon mieszczący się w
/// budżecie; usunięte tury zastępuje jedną notatką systemową (rola user).
/// Zwraca `true`, jeśli coś skompaktowano.
pub fn compact_history(history: &mut Vec<ChatMessage>, max_tokens: usize) -> bool {
    let total: usize = history.iter().map(estimate_tokens).sum();
    if total <= max_tokens || history.len() <= 2 {
        return false;
    }

    // Zachowaj pierwszą wiadomość użytkownika (indeks pierwszego user).
    let first_user = history
        .iter()
        .position(|m| m.role == Role::User)
        .unwrap_or(0);

    // Buduj ogon od końca, aż zmieści się w budżecie (rezerwa na notatkę + head).
    let budget = max_tokens.saturating_sub(estimate_tokens(&history[first_user]) + 32);
    let mut tail_start = history.len();
    let mut acc = 0usize;
    while tail_start > first_user + 1 {
        let candidate = tail_start - 1;
        let t = estimate_tokens(&history[candidate]);
        if acc + t > budget {
            break;
        }
        acc += t;
        tail_start = candidate;
    }

    // Nie rozbijaj pary: ogon nie może zaczynać się od wiadomości niosącej WYŁĄCZNIE
    // tool_results (odpowiedź na tool_use, który zostałby zwinięty).
    while tail_start < history.len()
        && !history[tail_start].tool_results.is_empty()
        && history[tail_start].tool_calls.is_empty()
        && history[tail_start].content.is_empty()
    {
        tail_start += 1;
    }

    // Jeśli nie ma czego zwijać między head a ogonem, zostaw bez zmian.
    if tail_start <= first_user + 1 {
        return false;
    }

    let dropped = tail_start - (first_user + 1);
    let note = ChatMessage {
        role: Role::User,
        content: format!(
            "[Skrót historii: pominięto {dropped} wcześniejszych wiadomości dla oszczędności kontekstu.]"
        ),
        tool_calls: vec![],
        tool_results: vec![],
    };

    let mut compacted = Vec::with_capacity(history.len() - dropped + 1);
    compacted.push(history[first_user].clone());
    compacted.push(note);
    compacted.extend_from_slice(&history[tail_start..]);
    *history = compacted;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ToolCall, ToolResult};

    fn user(t: &str) -> ChatMessage {
        ChatMessage::user(t)
    }
    fn assistant(t: &str) -> ChatMessage {
        ChatMessage::assistant(t)
    }

    #[test]
    fn no_compaction_under_budget() {
        let mut h = vec![user("cześć"), assistant("hej")];
        assert!(!compact_history(&mut h, 1000));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn compacts_long_history_keeping_head_and_tail() {
        let mut h = vec![user("ZADANIE: ustaw brzmienie")];
        for i in 0..40 {
            let long = format!("krok {i} — bardzo długa odpowiedź modelu ").repeat(10);
            h.push(assistant(&long));
            h.push(user(&format!("dalej {i}")));
        }
        let last = h.last().unwrap().content.clone();
        let did = compact_history(&mut h, 200);
        assert!(did);
        // Pierwsza wiadomość zachowana, jest notatka skrótu, ogon zachowany.
        assert!(h[0].content.starts_with("ZADANIE"));
        assert!(h[1].content.contains("Skrót historii"));
        assert_eq!(h.last().unwrap().content, last);
        assert!(h.len() < 81);
    }

    #[test]
    fn does_not_split_tool_use_result_pair() {
        // Historia: user, assistant(tool_call), user(tool_result), ... długi ogon.
        let mut h = vec![user("start")];
        for i in 0..30 {
            h.push(ChatMessage {
                role: Role::Assistant,
                content: format!("wywołuję narzędzie {i} ").repeat(20),
                tool_calls: vec![ToolCall {
                    id: format!("t{i}"),
                    name: "get_profile".into(),
                    arguments: Default::default(),
                }],
                tool_results: vec![],
            });
            h.push(ChatMessage {
                role: Role::User,
                content: String::new(),
                tool_calls: vec![],
                tool_results: vec![ToolResult {
                    tool_use_id: format!("t{i}"),
                    content: "wynik ".repeat(20),
                    is_error: false,
                }],
            });
        }
        compact_history(&mut h, 300);
        // Pierwsza wiadomość ogonu (po head + notatce) nie może być osieroconym
        // tool_result — to złamałoby parowanie tool_use↔tool_result w API.
        let first_tail = &h[2];
        let orphan = !first_tail.tool_results.is_empty()
            && first_tail.tool_calls.is_empty()
            && first_tail.content.is_empty();
        assert!(!orphan, "osierocony tool_result na początku ogonu");
    }
}
