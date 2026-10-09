//! File extension → tree-sitter language + bundled highlight queries.

use std::path::Path;
use std::sync::OnceLock;
use tree_sitter_highlight::HighlightConfiguration;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lang {
    Php,
    Js,
    Jsx,
    Ts,
    Tsx,
    Json,
    Html,
    Css,
    Markdown,
    Yaml,
    Plain,
}

impl Lang {
    pub fn from_path(p: &Path) -> Self {
        match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
            "php" | "phtml" | "inc" => Lang::Php,
            "js" | "mjs" | "cjs" => Lang::Js,
            "jsx" => Lang::Jsx,
            "ts" | "mts" | "cts" => Lang::Ts,
            "tsx" => Lang::Tsx,
            "json" | "jsonc" | "lock" => Lang::Json,
            "html" | "htm" | "vue" | "twig" | "blade" => Lang::Html,
            "css" | "scss" | "less" => Lang::Css,
            "md" | "markdown" => Lang::Markdown,
            "yml" | "yaml" => Lang::Yaml,
            _ => Lang::Plain,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Php => "PHP",
            Lang::Js => "JavaScript",
            Lang::Jsx => "JavaScript JSX",
            Lang::Ts => "TypeScript",
            Lang::Tsx => "TypeScript JSX",
            Lang::Json => "JSON",
            Lang::Html => "HTML",
            Lang::Css => "CSS",
            Lang::Markdown => "Markdown",
            Lang::Yaml => "YAML",
            Lang::Plain => "Plain text",
        }
    }

    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::Php | Lang::Js | Lang::Jsx | Lang::Ts | Lang::Tsx => Some("//"),
            Lang::Yaml => Some("#"),
            _ => None,
        }
    }

    /// Highlight configuration, built once per language. `None` for plain text.
    pub fn config(self) -> Option<&'static HighlightConfiguration> {
        static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; 10] = [const { OnceLock::new() }; 10];
        let idx = match self {
            Lang::Php => 0,
            Lang::Js => 1,
            Lang::Jsx => 2,
            Lang::Ts => 3,
            Lang::Tsx => 4,
            Lang::Json => 5,
            Lang::Html => 6,
            Lang::Css => 7,
            Lang::Markdown => 8,
            Lang::Yaml => 9,
            Lang::Plain => return None,
        };
        CONFIGS[idx].get_or_init(|| build(self)).as_ref()
    }
}

fn build(lang: Lang) -> Option<HighlightConfiguration> {
    use tree_sitter_javascript as js;
    use tree_sitter_typescript as ts;
    let jsx_highlights = format!("{}\n{}", js::JSX_HIGHLIGHT_QUERY, js::HIGHLIGHT_QUERY);
    let ts_highlights = format!("{}\n{}", ts::HIGHLIGHTS_QUERY, js::HIGHLIGHT_QUERY);
    let tsx_highlights = format!("{}\n{}\n{}", ts::HIGHLIGHTS_QUERY, js::JSX_HIGHLIGHT_QUERY, js::HIGHLIGHT_QUERY);
    let (language, name, hl, inj, loc): (tree_sitter::Language, &str, &str, &str, &str) = match lang {
        Lang::Php => (
            tree_sitter_php::LANGUAGE_PHP.into(),
            "php",
            tree_sitter_php::HIGHLIGHTS_QUERY,
            tree_sitter_php::INJECTIONS_QUERY,
            "",
        ),
        Lang::Js => (
            js::LANGUAGE.into(),
            "javascript",
            js::HIGHLIGHT_QUERY,
            js::INJECTIONS_QUERY,
            js::LOCALS_QUERY,
        ),
        Lang::Jsx => (js::LANGUAGE.into(), "jsx", &jsx_highlights, js::INJECTIONS_QUERY, js::LOCALS_QUERY),
        Lang::Ts => (
            ts::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            &ts_highlights,
            js::INJECTIONS_QUERY,
            ts::LOCALS_QUERY,
        ),
        Lang::Tsx => (
            ts::LANGUAGE_TSX.into(),
            "tsx",
            &tsx_highlights,
            js::INJECTIONS_QUERY,
            ts::LOCALS_QUERY,
        ),
        Lang::Json => (
            tree_sitter_json::LANGUAGE.into(),
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        Lang::Html => (
            tree_sitter_html::LANGUAGE.into(),
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY,
            tree_sitter_html::INJECTIONS_QUERY,
            "",
        ),
        Lang::Css => (tree_sitter_css::LANGUAGE.into(), "css", tree_sitter_css::HIGHLIGHTS_QUERY, "", ""),
        Lang::Markdown => (
            tree_sitter_md::LANGUAGE.into(),
            "markdown",
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            tree_sitter_md::INJECTION_QUERY_BLOCK,
            "",
        ),
        Lang::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            "yaml",
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
        Lang::Plain => return None,
    };
    match HighlightConfiguration::new(language, name, hl, inj, loc) {
        Ok(mut c) => {
            c.configure(super::highlight::NAMES);
            Some(c)
        }
        Err(e) => {
            eprintln!("highlight config for {name} failed: {e}");
            None
        }
    }
}

/// Injection callback: language name mentioned in an `injections.scm` → config.
pub fn by_name(name: &str) -> Option<&'static HighlightConfiguration> {
    match name {
        "php" => Lang::Php.config(),
        "javascript" | "js" => Lang::Js.config(),
        "typescript" | "ts" => Lang::Ts.config(),
        "tsx" => Lang::Tsx.config(),
        "json" => Lang::Json.config(),
        "html" => Lang::Html.config(),
        "css" => Lang::Css.config(),
        "yaml" => Lang::Yaml.config(),
        "markdown" => Lang::Markdown.config(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_config_builds() {
        for l in [
            Lang::Php,
            Lang::Js,
            Lang::Jsx,
            Lang::Ts,
            Lang::Tsx,
            Lang::Json,
            Lang::Html,
            Lang::Css,
            Lang::Markdown,
            Lang::Yaml,
        ] {
            assert!(l.config().is_some(), "{l:?}");
        }
        assert_eq!(Lang::from_path(Path::new("a/b/App.PHP")), Lang::Php);
        assert_eq!(Lang::from_path(Path::new("x.tsx")), Lang::Tsx);
        assert_eq!(Lang::from_path(Path::new("Makefile")), Lang::Plain);
    }
}
