//! Symbol extraction (classes, functions, methods…) via tree-sitter queries.

use crate::editor::languages::Lang;
use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub col: usize,
}

const PHP: &str = r#"
(class_declaration name: (name) @name) @class
(interface_declaration name: (name) @name) @interface
(trait_declaration name: (name) @name) @trait
(enum_declaration name: (name) @name) @enum
(function_definition name: (name) @name) @function
(method_declaration name: (name) @name) @method
"#;

const JS: &str = r#"
(class_declaration name: (identifier) @name) @class
(function_declaration name: (identifier) @name) @function
(method_definition name: (property_identifier) @name) @method
(variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @function
"#;

const TS: &str = r#"
(class_declaration name: (type_identifier) @name) @class
(abstract_class_declaration name: (type_identifier) @name) @class
(interface_declaration name: (type_identifier) @name) @interface
(type_alias_declaration name: (type_identifier) @name) @type
(enum_declaration name: (identifier) @name) @enum
(function_declaration name: (identifier) @name) @function
(method_definition name: (property_identifier) @name) @method
(variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @function
"#;

fn query_for(lang: Lang) -> Option<(&'static Query, tree_sitter::Language)> {
    static Q: [OnceLock<Option<(Query, tree_sitter::Language)>>; 5] = [const { OnceLock::new() }; 5];
    let (idx, language, src): (usize, tree_sitter::Language, &str) = match lang {
        Lang::Php => (0, tree_sitter_php::LANGUAGE_PHP.into(), PHP),
        Lang::Js | Lang::Jsx => (1, tree_sitter_javascript::LANGUAGE.into(), JS),
        Lang::Ts => (2, tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), TS),
        Lang::Tsx => (3, tree_sitter_typescript::LANGUAGE_TSX.into(), TS),
        _ => return None,
    };
    Q[idx]
        .get_or_init(|| match Query::new(&language, src) {
            Ok(q) => Some((q, language)),
            Err(e) => {
                eprintln!("symbol query for {lang:?}: {e}");
                None
            }
        })
        .as_ref()
        .map(|(q, l)| (q, l.clone()))
}

pub fn extract(lang: Lang, src: &[u8]) -> Vec<Symbol> {
    let Some((query, language)) = query_for(lang) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(src, None) else { return Vec::new() };
    let names = query.capture_names();
    let name_idx = names.iter().position(|n| *n == "name").unwrap_or(usize::MAX) as u32;
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let mut matches = cursor.matches(query, tree.root_node(), src);
    while let Some(m) = matches.next() {
        let mut name = None;
        let mut kind = None;
        for c in m.captures() {
            if c.index == name_idx {
                name = c.node.utf8_text(src).ok().map(|s| (s.to_owned(), c.node.start_position()));
            } else {
                kind = Some(names[c.index as usize]);
            }
        }
        if let (Some((name, pos)), Some(kind)) = (name, kind) {
            out.push(Symbol {
                name,
                kind: kind.to_owned(),
                line: pos.row,
                col: pos.column,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_and_ts_symbols() {
        let php =
            b"<?php\nnamespace A;\ninterface I {}\nclass Foo extends Bar {\n  public function go(): void {}\n}\nfunction helper() {}\n";
        let s = extract(Lang::Php, php);
        let got: Vec<(&str, &str, usize)> = s.iter().map(|x| (x.name.as_str(), x.kind.as_str(), x.line)).collect();
        assert_eq!(
            got,
            vec![
                ("I", "interface", 2),
                ("Foo", "class", 3),
                ("go", "method", 4),
                ("helper", "function", 6)
            ]
        );

        let ts = b"export interface P { a: number }\nexport class C { m() {} }\nconst f = () => 1;\nfunction g() {}\ntype T = string;\n";
        let s = extract(Lang::Ts, ts);
        let kinds: Vec<&str> = s.iter().map(|x| x.kind.as_str()).collect();
        assert_eq!(kinds, vec!["interface", "class", "method", "function", "function", "type"]);
        assert!(extract(Lang::Json, b"{}").is_empty());
    }
}
