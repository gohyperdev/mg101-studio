//! Diagnostyka ścieżki agenta BEZ GUI: odtwarza dokładnie to, co robi aplikacja —
//! ChatRunner na osobnym wątku + pompa wykonująca narzędzia na Studio — i wypisuje
//! każde żądanie narzędzia oraz zdarzenie końcowe.
//!
//! NIGDY nie wypisuje wartości klucza — tylko dostawcę i długość.
//!
//! Uruchomienie: `cargo run -p mg101-desktop --example agent_probe`

use mg101_agent_core::{AgentConfig, ChatMessage, Provider, Role};
use mg101_desktop::agent::{AgentEvent, ChatRunner, SYSTEM_PROMPT};
use mg101_library::MemoryStore;
use mg101_studio::Studio;

fn main() {
    let Some(key) = mg101_desktop::keychain::load_key(Provider::Anthropic) else {
        println!("Klucz z magazynu: BRAK — agent nie ruszy.");
        return;
    };
    println!("Klucz z magazynu: obecny, długość {}", key.len());

    let config = AgentConfig {
        provider: Provider::Anthropic,
        endpoint: "https://api.anthropic.com".into(),
        model: "claude-sonnet-5".into(),
        api_key: key,
    }
    .normalized();
    println!("Endpoint: {} | model: {}", config.endpoint, config.model);

    // Studio jak w aplikacji (profil MG-101 z packa).
    let (profile, catalog) = mg101_pack_nux_mg101::load().expect("profil MG-101");
    let mut studio = Studio::new(MemoryStore::new(), &profile, &catalog, 0);

    let history = vec![ChatMessage {
        role: Role::User,
        content: "Czy dasz radę przygotować ustawienia dla gitary basowej na MG101?".into(),
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
    }];

    let runner = ChatRunner::start(config, SYSTEM_PROMPT.to_string(), history);
    println!("--- przebieg wystartował, pompa działa ---");

    let start = std::time::Instant::now();
    loop {
        // Pompa narzędzi (jak Timer w UI).
        while let Some(cmd) = runner.try_tool_request() {
            let name = cmd.tool_name();
            let res = studio.execute(&cmd).map_err(|e| e.to_string());
            match &res {
                Ok(_) => println!("  narzędzie {name} → OK"),
                Err(e) => println!("  narzędzie {name} → BŁĄD: {e}"),
            }
            runner.send_tool_result(res);
        }
        if let Some(ev) = runner.try_event() {
            match ev {
                AgentEvent::Done { history, usage } => {
                    println!(
                        "--- KONIEC: tokeny in/out {}/{} ---",
                        usage.input_tokens, usage.output_tokens
                    );
                    if let Some(last) = history.last() {
                        println!("Ostatnia wiadomość ({:?}):\n{}", last.role, last.content);
                    }
                }
                AgentEvent::Error(e) => println!("--- BŁĄD AGENTA: {e} ---"),
            }
            return;
        }
        if start.elapsed() > std::time::Duration::from_secs(120) {
            println!("--- TIMEOUT: przebieg utknął (brak zdarzenia końcowego) ---");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}
