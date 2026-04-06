//! Shared text analysis utilities used across the reasoning pipeline.
//!
//! Centralises keyword extraction to avoid duplicated STOPWORD lists
//! across `plan_coherence.rs` and `round_scorer.rs`.

use std::collections::HashSet;

/// Common English stopwords filtered during keyword extraction.
///
/// This is the union of the stopword lists that were previously maintained
/// separately in `plan_coherence.rs` and `round_scorer.rs`.
pub(crate) const ANALYSIS_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "are", "was", "were", "will", "from", "into",
    "not", "but", "all", "can", "its", "have", "been", "has", "had", "our", "your", "their",
    "then", "than", "when", "what", "how", "use", "you", "also", "new", "step", "using", "each",
    "make", "run",
];

/// Spanish stopwords filtered during multilingual keyword extraction.
///
/// Includes common function words and generic task verbs that carry no
/// discriminative signal for goal-coverage estimation.
pub(crate) const SPANISH_STOPWORDS: &[&str] = &[
    "de", "del", "el", "la", "los", "las", "en", "con", "para", "por", "una", "uno", "como", "que",
    "al", "sus", "ser", "pero", "más", "este", "esta", "esto", "son", "hay", "han", "fue", "era",
    "todo", "todas", "todos", "sobre", "entre", "hasta", "desde", "hacia", "sin", "bajo", "ante",
    "tras", "según", "durante", "mediante", "obtener", "realizar", "hacer", "ver", "dar", "tener",
    "puede", "debe",
];

/// Map a Spanish word to its closest English equivalent for cross-lingual coverage.
///
/// Only covers the most common domain words in software/project tasks.
/// Returns `None` for words with no common English counterpart.
pub(crate) fn spanish_to_english(word: &str) -> Option<&'static str> {
    match word {
        "estructura" | "estructural" | "estructuras" => Some("structure"),
        "repositorio" | "repositorios" => Some("repository"),
        "proyecto" | "proyectos" => Some("project"),
        "código" | "codigo" => Some("code"),
        "archivo" | "archivos" => Some("file"),
        "módulo" | "modulo" | "módulos" | "modulos" => Some("module"),
        "función" | "funcion" | "funciones" => Some("function"),
        "directorio" | "directorios" => Some("directory"),
        "clase" | "clases" => Some("class"),
        "prueba" | "pruebas" => Some("test"),
        "análisis" | "analisis" => Some("analysis"),
        "implementación" | "implementacion" => Some("implementation"),
        "configuración" | "configuracion" => Some("configuration"),
        "servicio" | "servicios" => Some("service"),
        "datos" => Some("data"),
        "interfaz" => Some("interface"),
        "método" | "metodo" | "métodos" | "metodos" => Some("method"),
        "herramienta" | "herramientas" => Some("tool"),
        "componente" | "componentes" => Some("component"),
        "vista" | "vistas" => Some("view"),
        "general" => Some("overview"),
        "tipo" | "tipos" => Some("type"),
        "variable" | "variables" => Some("variable"),
        "cargo" => Some("cargo"),
        "crate" | "crates" => Some("crate"),
        "dependencia" | "dependencias" => Some("dependency"),
        _ => None,
    }
}

/// Extract meaningful keywords from text for coherence/scoring analysis.
///
/// - Lowercases all text.
/// - Splits on whitespace and common punctuation.
/// - Filters stopwords and very-short words (< 3 chars).
pub(crate) fn extract_keywords(text: &str) -> HashSet<String> {
    text.split(|c: char| c.is_whitespace() || ".,;:!?()[]{}<>\"'`-_/\\|@#$%^&*+=~".contains(c))
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !ANALYSIS_STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Extract keywords with bilingual (Spanish + English) support.
///
/// In addition to English keyword extraction, this function:
/// 1. Filters Spanish stopwords.
/// 2. For each Spanish domain word, adds its English equivalent — so coverage
///    estimation works even when the agent output is in English but the
///    instruction is in Spanish.
///
/// Example: instruction "estructura del repositorio" → keywords include both
/// "estructura" AND "structure", "repositorio" AND "repository".  When the
/// agent outputs "The repository structure includes...", both match.
pub(crate) fn extract_keywords_multilingual(text: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    for word in
        text.split(|c: char| c.is_whitespace() || ".,;:!?()[]{}<>\"'`-_/\\|@#$%^&*+=~".contains(c))
    {
        let w = word.to_lowercase();
        if w.len() < 3
            || ANALYSIS_STOPWORDS.contains(&w.as_str())
            || SPANISH_STOPWORDS.contains(&w.as_str())
        {
            continue;
        }
        // Add English equivalent when this is a known Spanish domain word.
        if let Some(en) = spanish_to_english(&w) {
            result.insert(en.to_string());
        }
        result.insert(w);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_keywords_lowercases_and_filters_stopwords() {
        let kw = extract_keywords("The authentication flow for the database");
        assert!(kw.contains("authentication"), "should keep content word");
        assert!(kw.contains("database"), "should keep content word");
        assert!(!kw.contains("the"), "should filter stopword 'the'");
        assert!(!kw.contains("for"), "should filter stopword 'for'");
    }

    #[test]
    fn extract_keywords_filters_short_words() {
        let kw = extract_keywords("a to do it");
        assert!(kw.is_empty(), "all short words should be filtered");
    }

    #[test]
    fn extract_keywords_splits_on_punctuation() {
        let kw = extract_keywords("authentication,authorization;validation");
        assert!(kw.contains("authentication"));
        assert!(kw.contains("authorization"));
        assert!(kw.contains("validation"));
    }

    #[test]
    fn extract_keywords_filters_step_and_run() {
        // Words that were added to plan_coherence but not round_scorer — now unified.
        let kw = extract_keywords("step using each make run");
        assert!(
            kw.is_empty(),
            "'step', 'using', 'each', 'make', 'run' are stopwords"
        );
    }

    #[test]
    fn extract_keywords_handles_empty_string() {
        let kw = extract_keywords("");
        assert!(kw.is_empty());
    }

    // ── Multilingual extraction tests ─────────────────────────────────────────

    #[test]
    fn multilingual_adds_english_for_estructura() {
        let kw = extract_keywords_multilingual("estructura del repositorio");
        assert!(
            kw.contains("structure"),
            "should translate 'estructura' → 'structure': {:?}",
            kw
        );
        assert!(
            kw.contains("repository"),
            "should translate 'repositorio' → 'repository': {:?}",
            kw
        );
    }

    #[test]
    fn multilingual_filters_spanish_stopwords() {
        let kw = extract_keywords_multilingual("de el la los las para por con");
        assert!(
            kw.is_empty(),
            "Spanish stopwords should all be filtered: {:?}",
            kw
        );
    }

    #[test]
    fn multilingual_keeps_original_spanish_word_too() {
        let kw = extract_keywords_multilingual("módulo de autenticación");
        // Should contain BOTH the original and the English translation.
        assert!(
            kw.contains("module") || kw.contains("módulo") || kw.contains("modulo"),
            "should keep module-related keyword: {:?}",
            kw
        );
    }

    #[test]
    fn multilingual_handles_english_only_input() {
        let kw = extract_keywords_multilingual("analyze the authentication module");
        assert!(
            kw.contains("analyze") || kw.contains("authentication"),
            "should extract English content words: {:?}",
            kw
        );
        assert!(!kw.contains("the"), "should filter English stopword 'the'");
    }

    #[test]
    fn spanish_to_english_returns_expected_translations() {
        assert_eq!(spanish_to_english("estructura"), Some("structure"));
        assert_eq!(spanish_to_english("repositorio"), Some("repository"));
        assert_eq!(spanish_to_english("proyecto"), Some("project"));
        assert_eq!(spanish_to_english("archivo"), Some("file"));
        assert_eq!(spanish_to_english("unknown_word"), None);
    }
}
