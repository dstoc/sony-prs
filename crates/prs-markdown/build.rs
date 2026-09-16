use std::env;
use std::path::PathBuf;

use syntect::parsing::{SyntaxDefinition, SyntaxSetBuilder};

/// Small TextMate grammars for the languages most often emitted by agents.
/// They intentionally cover comments, strings, numbers, declarations, and
/// language keywords rather than embedding Syntect's unrestricted package.
const GRAMMARS: &[(&str, &str, &str, &str)] = &[
    ("Bourne Shell", "sh", "source.shell", "#|if|then|else|fi|for|in|do|done|case|esac|function|export|local|echo|printf|set|unset"),
    ("Rust", "rs", "source.rust", "as|async|await|break|const|continue|crate|dyn|else|enum|extern|fn|for|if|impl|in|let|loop|match|mod|move|mut|pub|ref|return|self|Self|static|struct|super|trait|type|unsafe|use|where|while|abstract|become|box|do|final|macro|override|priv|typeof|unsized|virtual|yield|try"),
    ("Python", "py", "source.python", "and|as|assert|async|await|break|case|class|continue|def|del|elif|else|except|finally|for|from|global|if|import|in|is|lambda|match|nonlocal|not|or|pass|raise|return|try|while|with|yield|True|False|None"),
    ("JavaScript", "js", "source.js", "as|async|await|break|case|catch|class|const|continue|debugger|default|delete|do|else|export|extends|finally|for|from|function|get|if|import|in|instanceof|let|new|null|of|return|set|static|super|switch|this|throw|try|typeof|undefined|var|void|while|with|yield|true|false"),
    ("TypeScript", "ts", "source.ts", "as|async|await|break|case|catch|class|const|continue|debugger|declare|default|delete|do|else|enum|export|extends|finally|for|from|function|if|implements|import|in|infer|instanceof|interface|keyof|let|module|namespace|never|new|null|of|private|protected|public|readonly|return|static|string|super|switch|this|throw|type|typeof|undefined|unknown|var|void|while|with|yield|true|false"),
    ("JSON", "json", "source.json", "true|false|null"),
    ("YAML", "yaml", "source.yaml", "true|false|null|yes|no|on|off"),
    ("TOML", "toml", "source.toml", "true|false"),
    ("C", "c", "source.c", "auto|break|case|char|const|continue|default|do|double|else|enum|extern|float|for|goto|if|inline|int|long|register|restrict|return|short|signed|sizeof|static|struct|switch|typedef|union|unsigned|void|volatile|while"),
    ("C++", "cpp", "source.cpp", "alignas|alignof|and|asm|auto|bool|break|case|catch|char|class|const|constexpr|continue|default|delete|do|double|else|enum|explicit|export|extern|false|float|for|friend|if|inline|int|long|mutable|namespace|new|noexcept|nullptr|operator|or|private|protected|public|register|reinterpret_cast|return|short|signed|sizeof|static|struct|switch|template|this|throw|true|try|typedef|typename|union|unsigned|using|virtual|void|volatile|while"),
    ("Go", "go", "source.go", "break|default|func|interface|select|case|defer|go|map|struct|chan|else|goto|package|switch|const|fallthrough|if|range|type|continue|for|import|return|var|true|false|nil"),
    ("HTML", "html", "text.html", "DOCTYPE|html|head|body|script|style|div|span|a|p|h1|h2|h3|class|id|href|src"),
    ("CSS", "css", "source.css", "important|inherit|initial|unset|none|block|inline|flex|grid|absolute|relative|fixed|color|display|position|margin|padding|width|height"),
    ("SQL", "sql", "source.sql", "select|from|where|and|or|not|insert|into|values|update|set|delete|create|alter|drop|table|join|inner|left|right|outer|on|group|by|order|having|limit|as|null|true|false"),
    ("Diff", "diff", "source.diff", "diff|index|---|\\+\\+\\+|@@|old|new"),
    ("Markdown", "md", "text.html.markdown", "heading|link|image|code|blockquote|list|todo"),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut builder = SyntaxSetBuilder::new();
    for (name, extension, scope, keywords) in GRAMMARS {
        let source = grammar_source(name, extension, scope, keywords);
        let syntax = SyntaxDefinition::load_from_str(&source, true, Some(extension))
            .unwrap_or_else(|error| panic!("parse bundled {name} grammar: {error}\n{source}"));
        builder.add(syntax);
    }
    builder.add_plain_text_syntax();
    let selected = builder.build();

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"))
        .join("prs-markdown-syntaxes.packdump");
    syntect::dumps::dump_to_uncompressed_file(&selected, &output)
        .expect("serialize selected Syntect syntax set");

    let selected_bytes = std::fs::metadata(&output)
        .expect("stat selected Syntect syntax set")
        .len();
    println!(
        "cargo:warning=prs-markdown Syntect bundle: {} grammars, {} bytes",
        selected.syntaxes().len(),
        selected_bytes
    );
}

fn grammar_source(name: &str, extension: &str, scope: &str, keywords: &str) -> String {
    let comment = if name == "HTML" || name == "Markdown" {
        "<!--.*?-->"
    } else if name == "CSS" {
        "/\\*.*?\\*/"
    } else if name == "SQL"
        || name == "C"
        || name == "C++"
        || name == "JavaScript"
        || name == "TypeScript"
        || name == "Go"
        || name == "Rust"
    {
        "//.*$"
    } else {
        "#.*$"
    };
    let block_comment_start = if name == "Python" {
        "\"'''\""
    } else {
        "'/\\*'"
    };
    let block_comment_end = if name == "Python" {
        "\"'''\""
    } else {
        "'\\*/'"
    };
    let keyword_pattern = format!(r"\b({keywords})\b");
    format!(
        r#"---
name: {name}
file_extensions: [{extension}]
scope: {scope}
contexts:
  main:
    - match: '{comment}'
      scope: comment.line
    - match: {block_comment_start}
      push: block-comment
    - match: "\""
      push: string
    - match: "'"
      push: single-string
    - match: '{keyword_pattern}'
      scope: keyword
    - match: '\b[0-9]+(\.[0-9]+)?\b'
      scope: constant.numeric
  string:
    - meta_scope: string.quoted.double
      match: '"'
      pop: true
    - match: '\\"'
      scope: constant.character.escape
  single-string:
    - meta_scope: string.quoted.single
      match: "'"
      pop: true
  block-comment:
    - meta_scope: comment.block
      match: {block_comment_end}
      pop: true
"#,
    )
}
