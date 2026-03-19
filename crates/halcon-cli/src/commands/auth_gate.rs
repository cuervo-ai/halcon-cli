//! Frontier auth gate — shown when `halcon chat` starts with no authenticated provider.
//!
//! Detects the auth state of every supported provider (cenzontle, anthropic, openai,
//! deepseek, gemini, claude_code, ollama) and, if none have valid credentials, renders
//! an interactive crossterm UI that lets the user configure one before the session begins.
//!
//! Supports two authentication methods, selected automatically per provider:
//!   • **Browser / OAuth** — cenzontle (Cuervo SSO) and claude_code (claude.ai)
//!   • **API key**         — anthropic, openai, deepseek, gemini
//!
//! Works identically for classic (REPL) and TUI modes because the gate runs before
//! either the REPL or ratatui TUI is initialized.

use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute, queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor,
    },
    terminal::{self, Clear, ClearType},
};
use halcon_auth::KeyStore;
use halcon_core::types::AppConfig;
use std::io::{self, Write};

const SERVICE_NAME: &str = "halcon-cli";

// ── Auth method classification ───────────────────────────────────────────────

/// How a provider authenticates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFlow {
    Browser,
    ApiKey,
    NoAuth,
}

/// Authentication state as observed right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    Authenticated,
    Missing,
    NoAuthRequired,
}

/// Single row in the provider list.
#[derive(Clone)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub subtitle: &'static str,
    pub flow: AuthFlow,
    pub status: AuthStatus,
    /// Env var that carries the credential (API key providers).
    pub env_var: Option<&'static str>,
    /// OS keystore key name.
    pub keystore_key: Option<&'static str>,
    /// Short user-facing hint shown in the setup screen.
    pub hint: &'static str,
}

/// Outcome returned to `chat::run` after the gate finishes.
pub struct AuthGateOutcome {
    /// True if at least one credential was successfully saved during this session.
    pub credentials_added: bool,
}

// ── Credential probing ────────────────────────────────────────────────────────

fn has_env_or_keystore(env_var: &str, keystore_key: &str) -> bool {
    if std::env::var(env_var)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    KeyStore::new(SERVICE_NAME)
        .get_secret(keystore_key)
        .ok()
        .flatten()
        .is_some()
}

fn cenzontle_authenticated() -> bool {
    std::env::var("CENZONTLE_ACCESS_TOKEN")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
        || KeyStore::new(SERVICE_NAME)
            .get_secret("cenzontle:access_token")
            .ok()
            .flatten()
            .is_some()
}

fn claude_code_authenticated() -> bool {
    let bin = locate_claude_binary();
    if let Ok(out) = std::process::Command::new(&bin)
        .args(["auth", "status", "--json"])
        .output()
    {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            return v["loggedIn"].as_bool().unwrap_or(false);
        }
    }
    false
}

fn locate_claude_binary() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let p = format!("{home}/.local/bin/claude");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    "claude".to_string()
}

/// Build the ordered list of providers with current auth status.
pub fn probe_providers(_config: &AppConfig) -> Vec<ProviderEntry> {
    vec![
        ProviderEntry {
            id: "cenzontle",
            label: "Cenzontle",
            subtitle: "Cuervo Cloud · SSO",
            flow: AuthFlow::Browser,
            status: if cenzontle_authenticated() { AuthStatus::Authenticated } else { AuthStatus::Missing },
            env_var: Some("CENZONTLE_ACCESS_TOKEN"),
            keystore_key: Some("cenzontle:access_token"),
            hint: "Abre el navegador para el flujo SSO de Cuervo",
        },
        ProviderEntry {
            id: "anthropic",
            label: "Anthropic",
            subtitle: "Claude API · api key",
            flow: AuthFlow::ApiKey,
            status: if has_env_or_keystore("ANTHROPIC_API_KEY", "anthropic_api_key") { AuthStatus::Authenticated } else { AuthStatus::Missing },
            env_var: Some("ANTHROPIC_API_KEY"),
            keystore_key: Some("anthropic_api_key"),
            hint: "console.anthropic.com → API keys",
        },
        ProviderEntry {
            id: "openai",
            label: "OpenAI",
            subtitle: "GPT API · api key",
            flow: AuthFlow::ApiKey,
            status: if has_env_or_keystore("OPENAI_API_KEY", "openai_api_key") { AuthStatus::Authenticated } else { AuthStatus::Missing },
            env_var: Some("OPENAI_API_KEY"),
            keystore_key: Some("openai_api_key"),
            hint: "platform.openai.com → API keys",
        },
        ProviderEntry {
            id: "deepseek",
            label: "DeepSeek",
            subtitle: "DeepSeek API · api key",
            flow: AuthFlow::ApiKey,
            status: if has_env_or_keystore("DEEPSEEK_API_KEY", "deepseek_api_key") { AuthStatus::Authenticated } else { AuthStatus::Missing },
            env_var: Some("DEEPSEEK_API_KEY"),
            keystore_key: Some("deepseek_api_key"),
            hint: "platform.deepseek.com → API keys",
        },
        ProviderEntry {
            id: "gemini",
            label: "Google Gemini",
            subtitle: "Gemini API · api key",
            flow: AuthFlow::ApiKey,
            status: if has_env_or_keystore("GEMINI_API_KEY", "gemini_api_key") { AuthStatus::Authenticated } else { AuthStatus::Missing },
            env_var: Some("GEMINI_API_KEY"),
            keystore_key: Some("gemini_api_key"),
            hint: "aistudio.google.com → Get API key",
        },
        ProviderEntry {
            id: "claude_code",
            label: "Claude Code",
            subtitle: "claude.ai OAuth · browser",
            flow: AuthFlow::Browser,
            status: if claude_code_authenticated() { AuthStatus::Authenticated } else { AuthStatus::Missing },
            env_var: None,
            keystore_key: None,
            hint: "Requiere el binario `claude` instalado",
        },
        ProviderEntry {
            id: "ollama",
            label: "Ollama",
            subtitle: "servidor local · sin auth",
            flow: AuthFlow::NoAuth,
            status: AuthStatus::NoAuthRequired,
            env_var: None,
            keystore_key: None,
            hint: "Inicia con: ollama serve",
        },
    ]
}

/// True if at least one provider that requires credentials has them.
pub fn any_authenticated(entries: &[ProviderEntry]) -> bool {
    entries
        .iter()
        .any(|p| p.status == AuthStatus::Authenticated)
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Check auth state and, if needed, run the interactive gate.
///
/// `registry_has_no_real_providers` — true when every API-requiring provider
/// is missing from the registry (only echo/ollama at most).
pub async fn run_if_needed(
    config: &AppConfig,
    registry_has_no_real_providers: bool,
) -> Result<AuthGateOutcome> {
    if !registry_has_no_real_providers {
        return Ok(AuthGateOutcome { credentials_added: false });
    }

    let providers = probe_providers(config);

    // If tokens exist somewhere (keystore, env) but registry is empty, the issue is
    // a config mismatch — let precheck_providers_explicit handle it with its own message.
    if any_authenticated(&providers) {
        return Ok(AuthGateOutcome { credentials_added: false });
    }

    // Don't show the interactive gate in non-TTY environments (CI, pipes).
    if !crossterm::tty::IsTty::is_tty(&io::stdin()) {
        return Ok(AuthGateOutcome { credentials_added: false });
    }

    run_gate(config, providers).await
}

// ── Interactive gate ──────────────────────────────────────────────────────────

const BOX_WIDTH: u16 = 66;

async fn run_gate(config: &AppConfig, providers: Vec<ProviderEntry>) -> Result<AuthGateOutcome> {
    let _ = config; // reserved for future use

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, Hide)?;

    let mut selected: usize = 0;
    let mut status_line = String::new();
    let mut status_ok = false;
    let mut credentials_added = false;

    'outer: loop {
        render_selector(&mut stdout, &providers, selected, &status_line, status_ok)?;

        match event::read()? {
            Event::Key(key) => {
                match key.code {
                    // Navigation
                    KeyCode::Up | KeyCode::Char('k') => {
                        if selected > 0 {
                            selected -= 1;
                        }
                        status_line.clear();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if selected + 1 < providers.len() {
                            selected += 1;
                        }
                        status_line.clear();
                    }

                    // Confirm
                    KeyCode::Enter => {
                        let entry = &providers[selected];
                        match entry.flow {
                            AuthFlow::ApiKey => {
                                // Tear down selector, show API key input screen
                                match run_api_key_input(&mut stdout, entry).await {
                                    Ok(true) => {
                                        credentials_added = true;
                                        break 'outer;
                                    }
                                    Ok(false) => {
                                        status_line = "Configuración cancelada.".into();
                                        status_ok = false;
                                    }
                                    Err(e) => {
                                        status_line = format!("Error: {e}");
                                        status_ok = false;
                                    }
                                }
                            }
                            AuthFlow::Browser => {
                                // Drop raw mode, run browser flow, re-enter
                                terminal::disable_raw_mode()?;
                                execute!(stdout, Show, MoveTo(0, 0), Clear(ClearType::All))?;

                                match run_browser_flow(entry).await {
                                    Ok(true) => {
                                        credentials_added = true;
                                        break 'outer;
                                    }
                                    Ok(false) => {
                                        terminal::enable_raw_mode()?;
                                        execute!(stdout, Hide)?;
                                        status_line = "Login no completado. Intenta de nuevo.".into();
                                        status_ok = false;
                                    }
                                    Err(e) => {
                                        terminal::enable_raw_mode()?;
                                        execute!(stdout, Hide)?;
                                        status_line = format!("Error: {e}");
                                        status_ok = false;
                                    }
                                }
                            }
                            AuthFlow::NoAuth => {
                                // Show Ollama instructions and skip
                                terminal::disable_raw_mode()?;
                                execute!(stdout, Show, MoveTo(0, 0), Clear(ClearType::All))?;
                                show_noauth_instructions(entry)?;
                                break 'outer;
                            }
                        }
                    }

                    // Skip
                    KeyCode::Esc
                    | KeyCode::Char('s')
                    | KeyCode::Char('S') => break 'outer,

                    // Ctrl+C / Ctrl+D
                    KeyCode::Char('c') | KeyCode::Char('d')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        break 'outer
                    }

                    _ => {}
                }
            }
            Event::Resize(_, _) => {} // just redraw next iteration
            _ => {}
        }
    }

    let _ = terminal::disable_raw_mode();
    let _ = execute!(stdout, Show);

    if credentials_added {
        println!();
        print_styled(&mut stdout, Color::Green, "  ✓ Proveedor configurado. Iniciando sesión...\n")?;
        stdout.flush()?;
        // Small pause so the user sees the success message.
        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    Ok(AuthGateOutcome { credentials_added })
}

// ── Selector screen ───────────────────────────────────────────────────────────

fn render_selector(
    stdout: &mut impl Write,
    providers: &[ProviderEntry],
    selected: usize,
    status: &str,
    status_ok: bool,
) -> Result<()> {
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;

    // Header
    box_top(stdout)?;
    box_line_text(stdout, "", Color::White)?;
    box_line_styled(stdout, "  halcon — configuración de proveedor", Color::Cyan, true)?;
    box_line_text(stdout, "", Color::White)?;
    box_line_text(stdout, "  No hay ningún proveedor de IA autenticado.", Color::White)?;
    box_line_text(stdout, "  Selecciona uno para comenzar:", Color::DarkGrey)?;
    box_line_text(stdout, "", Color::White)?;
    box_divider(stdout)?;
    box_line_text(stdout, "", Color::White)?;

    // Provider rows
    for (i, entry) in providers.iter().enumerate() {
        let is_selected = i == selected;
        let prefix = if is_selected { "  ❯ " } else { "    " };

        let (status_icon, status_color) = match entry.status {
            AuthStatus::Authenticated => ("●", Color::Green),
            AuthStatus::NoAuthRequired => ("○", Color::DarkGrey),
            AuthStatus::Missing => ("○", Color::DarkGrey),
        };

        let flow_tag = match entry.flow {
            AuthFlow::Browser => "[browser]",
            AuthFlow::ApiKey  => "[api key]",
            AuthFlow::NoAuth  => "[no auth]",
        };

        // Label column (fixed 16 chars), subtitle column (fixed 24 chars), tag
        let label_padded = format!("{:<16}", entry.label);
        let sub_padded   = format!("{:<26}", entry.subtitle);

        // Compose the full row content (inside the box)
        let row = format!("{prefix}{status_icon} {label_padded}{sub_padded}{flow_tag}");
        let row_display = format!("{:<62}", row);

        let line_color = if is_selected { Color::White } else { Color::DarkGrey };
        let label_color = if is_selected { Color::Cyan } else { Color::DarkGrey };

        queue!(
            stdout,
            Print("│ "),
            SetForegroundColor(if is_selected { Color::Cyan } else { Color::DarkGrey }),
            SetAttribute(if is_selected { Attribute::Bold } else { Attribute::Reset }),
        )?;

        // prefix + status icon
        queue!(
            stdout,
            Print(prefix),
            SetForegroundColor(status_color),
            Print(status_icon),
            ResetColor,
        )?;

        // label
        queue!(
            stdout,
            Print(" "),
            SetForegroundColor(label_color),
            SetAttribute(if is_selected { Attribute::Bold } else { Attribute::Reset }),
            Print(&label_padded),
            ResetColor,
        )?;

        // subtitle
        queue!(
            stdout,
            SetForegroundColor(if is_selected { Color::White } else { Color::DarkGrey }),
            Print(&sub_padded),
            ResetColor,
        )?;

        // flow tag
        let tag_color = match entry.flow {
            AuthFlow::Browser => if is_selected { Color::Yellow } else { Color::DarkGrey },
            AuthFlow::ApiKey  => if is_selected { Color::Blue }   else { Color::DarkGrey },
            AuthFlow::NoAuth  => Color::DarkGrey,
        };
        queue!(
            stdout,
            SetForegroundColor(tag_color),
            Print(flow_tag),
            ResetColor,
            Print(" │\n"),
        )?;
    }

    box_line_text(stdout, "", Color::White)?;
    box_divider(stdout)?;

    // Status line
    if !status.is_empty() {
        let color = if status_ok { Color::Green } else { Color::Red };
        box_line_styled(stdout, &format!("  {status}"), color, false)?;
    } else {
        box_line_text(stdout, "  [↑/↓] navegar  [Enter] configurar  [S] omitir", Color::DarkGrey)?;
    }

    box_bottom(stdout)?;
    stdout.flush()?;
    Ok(())
}

// ── API key input screen ──────────────────────────────────────────────────────

async fn run_api_key_input(
    stdout: &mut impl Write,
    entry: &ProviderEntry,
) -> Result<bool> {
    let mut input = String::new();
    let mut err_msg = String::new();

    loop {
        render_api_key_screen(stdout, entry, &input, &err_msg)?;

        match event::read()? {
            Event::Key(key) => match key.code {
                KeyCode::Enter => {
                    let key_val = input.trim().to_string();
                    if key_val.is_empty() {
                        err_msg = "No ingresaste ninguna clave.".into();
                        continue;
                    }
                    match save_api_key(entry, &key_val) {
                        Ok(()) => return Ok(true),
                        Err(e) => {
                            err_msg = format!("Error al guardar: {e}");
                        }
                    }
                }
                KeyCode::Esc => return Ok(false),
                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if c == 'c' || c == 'd' { return Ok(false); }
                    if c == 'u' { input.clear(); }
                    if c == 'w' {
                        // Delete last word
                        let trimmed = input.trim_end().to_string();
                        let word_end = trimmed.rfind(|c: char| c.is_whitespace()).map(|i| i + 1).unwrap_or(0);
                        input = trimmed[..word_end].to_string();
                    }
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    err_msg.clear();
                }
                KeyCode::Backspace => {
                    input.pop();
                    err_msg.clear();
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn render_api_key_screen(
    stdout: &mut impl Write,
    entry: &ProviderEntry,
    input: &str,
    err: &str,
) -> Result<()> {
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;

    box_top(stdout)?;
    box_line_text(stdout, "", Color::White)?;
    box_line_styled(stdout, &format!("  {} — API Key", entry.label), Color::Cyan, true)?;
    box_line_text(stdout, "", Color::White)?;
    box_line_text(stdout, &format!("  Obtén tu clave en: {}", entry.hint), Color::DarkGrey)?;
    box_line_text(stdout, "", Color::White)?;

    // Input field — mask with bullets
    let masked: String = "●".repeat(input.len().min(48));
    let cursor_visible = if input.is_empty() { "▌" } else { "" };
    let field = format!("  Clave: {masked}{cursor_visible}");
    box_line_styled(stdout, &field, Color::White, false)?;

    box_line_text(stdout, "", Color::White)?;

    // Env var hint
    if let Some(env) = entry.env_var {
        box_line_text(
            stdout,
            &format!("  También puedes exportar: {env}=<tu_clave>"),
            Color::DarkGrey,
        )?;
    }

    box_line_text(
        stdout,
        "  La clave se guarda de forma segura en el OS keystore.",
        Color::DarkGrey,
    )?;
    box_line_text(stdout, "", Color::White)?;
    box_divider(stdout)?;

    if !err.is_empty() {
        box_line_styled(stdout, &format!("  {err}"), Color::Red, false)?;
    } else {
        box_line_text(stdout, "  [Enter] guardar  [Ctrl+U] limpiar  [Esc] volver", Color::DarkGrey)?;
    }

    box_bottom(stdout)?;
    stdout.flush()?;
    Ok(())
}

fn save_api_key(entry: &ProviderEntry, api_key: &str) -> Result<()> {
    // Save to OS keystore
    if let Some(ks_key) = entry.keystore_key {
        KeyStore::new(SERVICE_NAME)
            .set_secret(ks_key, api_key)
            .map_err(|e| anyhow::anyhow!("keystore error: {e}"))?;
    }

    // Also set the env var for the current process so the rebuilt registry picks it up
    // immediately without needing a restart.
    if let Some(env_var) = entry.env_var {
        // Safety: we are the only writer in this process at this point.
        unsafe { std::env::set_var(env_var, api_key); }
    }

    Ok(())
}

// ── Browser OAuth flow ────────────────────────────────────────────────────────

/// Returns `true` if the browser flow completed successfully.
async fn run_browser_flow(entry: &ProviderEntry) -> Result<bool> {
    match entry.id {
        "cenzontle" => {
            // sso::login() handles the full PKCE OAuth flow, prints its own UI,
            // and stores the token in the keystore on success.
            match super::sso::login().await {
                Ok(()) => Ok(true),
                Err(e) => {
                    eprintln!("\n  Error durante el login: {e}");
                    Ok(false)
                }
            }
        }
        "claude_code" => {
            // auth::login("claude_code") handles the Claude.ai OAuth flow.
            match super::auth::login("claude_code") {
                Ok(()) => Ok(true),
                Err(e) => {
                    eprintln!("\n  Error durante el login: {e}");
                    Ok(false)
                }
            }
        }
        _ => Ok(false),
    }
}

// ── Ollama instructions ───────────────────────────────────────────────────────

fn show_noauth_instructions(entry: &ProviderEntry) -> Result<()> {
    let mut stdout = io::stdout();
    println!();
    print_styled(&mut stdout, Color::Cyan, "  Ollama — servidor local\n")?;
    println!();
    println!("  Ollama no requiere autenticación, pero el servidor debe estar");
    println!("  corriendo localmente antes de iniciar halcon.");
    println!();
    print_styled(&mut stdout, Color::Yellow, &format!("  {}\n", entry.hint))?;
    println!();
    println!("  Después de iniciar Ollama, ejecuta `halcon chat` de nuevo.");
    println!();
    stdout.flush()?;
    Ok(())
}

// ── Box drawing helpers ───────────────────────────────────────────────────────

fn box_top(stdout: &mut impl Write) -> Result<()> {
    queue!(
        stdout,
        SetForegroundColor(Color::DarkCyan),
        Print(format!("╭{:─<width$}╮\n", "", width = BOX_WIDTH as usize)),
        ResetColor,
    )?;
    Ok(())
}

fn box_bottom(stdout: &mut impl Write) -> Result<()> {
    queue!(
        stdout,
        SetForegroundColor(Color::DarkCyan),
        Print(format!("╰{:─<width$}╯\n", "", width = BOX_WIDTH as usize)),
        ResetColor,
    )?;
    Ok(())
}

fn box_divider(stdout: &mut impl Write) -> Result<()> {
    queue!(
        stdout,
        SetForegroundColor(Color::DarkCyan),
        Print(format!("├{:─<width$}┤\n", "", width = BOX_WIDTH as usize)),
        ResetColor,
    )?;
    Ok(())
}

fn box_line_text(stdout: &mut impl Write, text: &str, color: Color) -> Result<()> {
    // Truncate to fit inside the box
    let inner = BOX_WIDTH as usize;
    let padded = format!("{:<width$}", text, width = inner);
    // Truncate if the text (with padding) exceeds box width
    let safe: String = padded.chars().take(inner).collect();
    queue!(
        stdout,
        SetForegroundColor(Color::DarkCyan),
        Print("│"),
        SetForegroundColor(color),
        Print(&safe),
        SetForegroundColor(Color::DarkCyan),
        Print("│\n"),
        ResetColor,
    )?;
    Ok(())
}

fn box_line_styled(stdout: &mut impl Write, text: &str, color: Color, bold: bool) -> Result<()> {
    let inner = BOX_WIDTH as usize;
    let padded = format!("{:<width$}", text, width = inner);
    let safe: String = padded.chars().take(inner).collect();

    queue!(stdout, SetForegroundColor(Color::DarkCyan), Print("│"))?;
    if bold {
        queue!(stdout, SetAttribute(Attribute::Bold))?;
    }
    queue!(
        stdout,
        SetForegroundColor(color),
        Print(&safe),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkCyan),
        Print("│\n"),
        ResetColor,
    )?;
    Ok(())
}

fn print_styled(stdout: &mut impl Write, color: Color, text: &str) -> Result<()> {
    queue!(stdout, SetForegroundColor(color), Print(text), ResetColor)?;
    stdout.flush()?;
    Ok(())
}

// ── Registry empty check ──────────────────────────────────────────────────────

/// Returns true when no real AI provider is registered (only echo / ollama counts as "no real provider").
pub fn registry_has_no_real_providers(list: &[&str]) -> bool {
    list.iter()
        .all(|n| *n == "echo" || *n == "ollama")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_empty_check_true_for_echo_only() {
        assert!(registry_has_no_real_providers(&["echo"]));
    }

    #[test]
    fn registry_empty_check_false_when_anthropic_present() {
        assert!(!registry_has_no_real_providers(&["echo", "anthropic"]));
    }

    #[test]
    fn registry_empty_check_true_for_empty() {
        assert!(registry_has_no_real_providers(&[]));
    }

    #[test]
    fn registry_empty_check_false_for_cenzontle() {
        assert!(!registry_has_no_real_providers(&["cenzontle", "echo"]));
    }

    #[test]
    fn registry_empty_check_ollama_alone_counts_as_empty() {
        // Ollama is a local fallback but not an API provider requiring auth setup.
        assert!(registry_has_no_real_providers(&["ollama", "echo"]));
    }

    #[test]
    fn any_authenticated_false_when_all_missing() {
        let providers = vec![
            ProviderEntry {
                id: "anthropic", label: "Anthropic", subtitle: "", flow: AuthFlow::ApiKey,
                status: AuthStatus::Missing, env_var: None, keystore_key: None, hint: "",
            },
            ProviderEntry {
                id: "openai", label: "OpenAI", subtitle: "", flow: AuthFlow::ApiKey,
                status: AuthStatus::Missing, env_var: None, keystore_key: None, hint: "",
            },
        ];
        assert!(!any_authenticated(&providers));
    }

    #[test]
    fn any_authenticated_true_when_one_authenticated() {
        let providers = vec![
            ProviderEntry {
                id: "anthropic", label: "Anthropic", subtitle: "", flow: AuthFlow::ApiKey,
                status: AuthStatus::Authenticated, env_var: None, keystore_key: None, hint: "",
            },
            ProviderEntry {
                id: "openai", label: "OpenAI", subtitle: "", flow: AuthFlow::ApiKey,
                status: AuthStatus::Missing, env_var: None, keystore_key: None, hint: "",
            },
        ];
        assert!(any_authenticated(&providers));
    }

    #[test]
    fn provider_list_has_expected_entries() {
        use halcon_core::types::AppConfig;
        let config = AppConfig::default();
        let providers = probe_providers(&config);
        let ids: Vec<&str> = providers.iter().map(|p| p.id).collect();
        assert!(ids.contains(&"cenzontle"));
        assert!(ids.contains(&"anthropic"));
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"deepseek"));
        assert!(ids.contains(&"gemini"));
        assert!(ids.contains(&"claude_code"));
        assert!(ids.contains(&"ollama"));
    }

    #[test]
    fn ollama_is_always_no_auth_required() {
        use halcon_core::types::AppConfig;
        let config = AppConfig::default();
        let providers = probe_providers(&config);
        let ollama = providers.iter().find(|p| p.id == "ollama").unwrap();
        assert_eq!(ollama.status, AuthStatus::NoAuthRequired);
        assert_eq!(ollama.flow, AuthFlow::NoAuth);
    }
}
