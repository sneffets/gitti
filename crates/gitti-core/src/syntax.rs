//! Syntax highlighting without a UI toolkit: tree-sitter grammars + their highlight queries,
//! reduced to a few [`Category`]s that each frontend colours itself. Whole file versions are
//! parsed, so multi-line comments / strings that start outside a hunk are right; the result
//! is bucketed per line. Both apps use it (gitti: diffs and, through an
//! `InputHighlighter`, the merge editor's result pane).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator as _};

/// What a highlighted piece of code is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Comment,
    String,
    Keyword,
    Function,
    Type,
    Constant,
    Number,
    Property,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Heading,
}

impl Category {
    /// From a tree-sitter capture name (`keyword.function`, `string.special`, …).
    pub fn from_capture(name: &str) -> Option<Category> {
        let head = name.split('.').next().unwrap_or(name);
        Some(match (head, name) {
            ("comment", _) => Category::Comment,
            ("string" | "character" | "escape" | "regex", _) => Category::String,
            (
                "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass"
                | "preproc" | "define",
                _,
            ) => Category::Keyword,
            ("function" | "method" | "constructor", _) => Category::Function,
            ("type" | "class" | "namespace" | "module", _) => Category::Type,
            ("number" | "float", _) => Category::Number,
            ("boolean" | "constant", _) => Category::Constant,
            (_, "variable.builtin") => Category::Constant,
            ("property" | "field", _) => Category::Property,
            ("operator", _) => Category::Operator,
            ("punctuation", _) => Category::Punctuation,
            ("tag", _) => Category::Tag,
            ("attribute" | "label" | "annotation", _) => Category::Attribute,
            ("title" | "heading" | "markup", _) | (_, "text.title") => Category::Heading,
            _ => return None,
        })
    }
}

/// Highlighted ranges of one line (byte ranges within the line, without its `\n`).
pub type LineCategories = Vec<(Range<usize>, Category)>;

/// Tree-sitter language name for a path; `None` for unknown file types.
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_lowercase();
    match lower.as_str() {
        "makefile" | "gnumakefile" => return Some("make"),
        "cmakelists.txt" => return Some("cmake"),
        "cargo.lock" => return Some("toml"),
        ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => return Some("bash"),
        ".gitconfig" | ".gitmodules" | ".editorconfig" => return Some("ini"),
        _ => {}
    }
    let ext = lower.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => "rust",
        "py" | "pyi" | "pyw" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" | "json5" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" | "mdx" => "markdown",
        "c" => "c",
        // Headers are far more often C++ than plain C; the C++ grammar handles C fine.
        "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "inl"
        | "tpp" | "ixx" | "cppm" | "cu" | "cuh" | "metal" => "cpp",
        "m" | "mm" => "objc",
        "glsl" | "vert" | "frag" | "geom" | "tesc" | "tese" | "comp" | "vs" | "fs" | "vsh"
        | "fsh" | "rgen" | "rchit" | "rmiss" | "rahit" | "rint" | "rcall" | "mesh" | "task" => {
            "glsl"
        }
        "hlsl" | "hlsli" | "fx" | "fxh" | "usf" | "ush" => "hlsl",
        "wgsl" => "wgsl",
        "xml" | "xsd" | "xsl" | "xslt" | "svg" | "plist" | "csproj" | "vcxproj" | "props"
        | "targets" | "xaml" | "storyboard" | "xib" | "resx" | "manifest" => "xml",
        "ps1" | "psm1" | "psd1" => "powershell",
        "ini" | "cfg" | "inf" | "gitconfig" => "ini",
        "cs" => "csharp",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "zig" => "zig",
        "scala" | "sc" => "scala",
        "sh" | "bash" | "zsh" => "bash",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "sql" => "sql",
        "proto" => "proto",
        "graphql" | "gql" => "graphql",
        "ex" | "exs" => "elixir",
        "svelte" => "svelte",
        "astro" => "astro",
        "cmake" => "cmake",
        "mk" => "make",
        "diff" | "patch" => "diff",
        _ => return None,
    })
}

/// Grammar and highlight query of a language this module supports.
fn grammar(lang: &str) -> Option<(Language, String)> {
    let c = tree_sitter_c::HIGHLIGHT_QUERY;
    // Grammars that extend another one ship only their additions; the more specific
    // patterns go first (the first capture of a range wins).
    let c_family = || format!("{}\n{c}", tree_sitter_cpp::HIGHLIGHT_QUERY);
    let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let ts = || format!("{}\n{js}", tree_sitter_typescript::HIGHLIGHTS_QUERY);
    Some(match lang {
        "rust" => (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY.into()),
        "python" => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.into(),
        ),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            format!("{}\n{js}", tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
        ),
        "typescript" => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), ts()),
        "tsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), ts()),
        "json" => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into()),
        "toml" => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
        ),
        "yaml" => (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into()),
        "c" => (tree_sitter_c::LANGUAGE.into(), c.into()),
        "cpp" => (tree_sitter_cpp::LANGUAGE.into(), c_family()),
        // The HLSL grammar extends the C++ one and ships no queries.
        "hlsl" => (tree_sitter_hlsl::LANGUAGE_HLSL.into(), c_family()),
        // Already contains the C query it inherits.
        "glsl" => (
            tree_sitter_glsl::LANGUAGE_GLSL.into(),
            tree_sitter_glsl::HIGHLIGHTS_QUERY.into(),
        ),
        "objc" => (
            tree_sitter_objc::LANGUAGE.into(),
            format!("{}\n{c}", tree_sitter_objc::HIGHLIGHTS_QUERY),
        ),
        "wgsl" => (
            tree_sitter_wgsl_bevy::LANGUAGE.into(),
            include_str!("../queries/wgsl.scm").into(),
        ),
        "xml" => (
            tree_sitter_xml::LANGUAGE_XML.into(),
            tree_sitter_xml::XML_HIGHLIGHT_QUERY.into(),
        ),
        "powershell" => (
            tree_sitter_powershell::LANGUAGE.into(),
            tree_sitter_powershell::HIGHLIGHTS_QUERY.into(),
        ),
        "ini" => (tree_sitter_ini::LANGUAGE.into(), tree_sitter_ini::HIGHLIGHTS_QUERY.into()),
        "csharp" => (
            tree_sitter_c_sharp::LANGUAGE.into(),
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
        ),
        "go" => (tree_sitter_go::LANGUAGE.into(), tree_sitter_go::HIGHLIGHTS_QUERY.into()),
        "java" => (tree_sitter_java::LANGUAGE.into(), tree_sitter_java::HIGHLIGHTS_QUERY.into()),
        "bash" => (tree_sitter_bash::LANGUAGE.into(), tree_sitter_bash::HIGHLIGHT_QUERY.into()),
        "html" => (tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY.into()),
        "css" => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into()),
        "ruby" => (tree_sitter_ruby::LANGUAGE.into(), tree_sitter_ruby::HIGHLIGHTS_QUERY.into()),
        "lua" => (tree_sitter_lua::LANGUAGE.into(), tree_sitter_lua::HIGHLIGHTS_QUERY.into()),
        "zig" => (tree_sitter_zig::LANGUAGE.into(), tree_sitter_zig::HIGHLIGHTS_QUERY.into()),
        "scala" => (
            tree_sitter_scala::LANGUAGE.into(),
            tree_sitter_scala::HIGHLIGHTS_QUERY.into(),
        ),
        "sql" => (
            tree_sitter_sequel::LANGUAGE.into(),
            tree_sitter_sequel::HIGHLIGHTS_QUERY.into(),
        ),
        "make" => (tree_sitter_make::LANGUAGE.into(), tree_sitter_make::HIGHLIGHTS_QUERY.into()),
        "cmake" => (
            tree_sitter_cmake::LANGUAGE.into(),
            tree_sitter_cmake::HIGHLIGHTS_QUERY.into(),
        ),
        "swift" => (
            tree_sitter_swift::LANGUAGE.into(),
            tree_sitter_swift::HIGHLIGHTS_QUERY.into(),
        ),
        "diff" => (tree_sitter_diff::LANGUAGE.into(), tree_sitter_diff::HIGHLIGHTS_QUERY.into()),
        _ => return None,
    })
}

struct Compiled {
    language: Language,
    query: Query,
    /// Category per capture index.
    categories: Vec<Option<Category>>,
}

/// Compiled grammars, built once per language (a failed one is remembered as `None`).
fn compiled(lang: &str) -> Option<Arc<Compiled>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Compiled>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok()?.get(lang) {
        return hit.clone();
    }
    let built = grammar(lang).and_then(|(language, source)| {
        let query = Query::new(&language, &source).ok()?;
        let categories = query
            .capture_names()
            .iter()
            .map(|n| Category::from_capture(n))
            .collect();
        Some(Arc::new(Compiled {
            language,
            query,
            categories,
        }))
    });
    cache.lock().ok()?.insert(lang.to_string(), built.clone());
    built
}

/// Highlights `text` as `lang`; one entry per line (`None` for unsupported languages).
pub fn highlight(lang: &str, text: &str) -> Option<Vec<LineCategories>> {
    let c = compiled(lang)?;
    let mut parser = Parser::new();
    parser.set_language(&c.language).ok()?;
    let tree = parser.parse(text, None)?;
    // One category per byte; the first capture of a range wins (pattern order, then
    // position), so the specific patterns listed first beat the generic ones.
    let mut by_byte: Vec<Option<(u32, Category)>> = vec![None; text.len()];
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&c.query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let Some(cat) = c.categories.get(cap.index as usize).copied().flatten() else {
                continue;
            };
            let r = cap.node.byte_range();
            for slot in &mut by_byte[r.start.min(text.len())..r.end.min(text.len())] {
                match slot {
                    Some((p, _)) if *p <= m.pattern_index as u32 => {}
                    _ => *slot = Some((m.pattern_index as u32, cat)),
                }
            }
        }
    }
    let mut lines = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let mut runs: LineCategories = Vec::new();
        for (i, slot) in by_byte[start..start + line.len()].iter().enumerate() {
            let Some((_, cat)) = slot else { continue };
            match runs.last_mut() {
                Some((r, c)) if r.end == i && c == cat => r.end += 1,
                _ => runs.push((i..i + 1, *cat)),
            }
        }
        lines.push(runs);
        start += line.len() + 1;
    }
    Some(lines)
}

/// Maps styles of a raw line onto its displayed segment (tabs expanded to four spaces, `\r`
/// dropped, wrapped part `[char_start, char_end)` of the expanded text).
pub fn segment_ranges<T: Clone + PartialEq>(
    raw: &str,
    styles: &[(Range<usize>, T)],
    char_start: usize,
    char_end: usize,
) -> Vec<(Range<usize>, T)> {
    if styles.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(Range<usize>, T)> = Vec::new();
    let mut disp_char = 0usize;
    let mut seg_byte = 0usize;
    let mut style_idx = 0usize;
    for (raw_off, ch) in raw.char_indices() {
        let (width, disp_len) = match ch {
            '\t' => (4, 4),
            '\r' => (0, 0),
            c => (1, c.len_utf8()),
        };
        for _ in 0..width {
            let visible = disp_char >= char_start && disp_char < char_end;
            if visible {
                while style_idx < styles.len() && styles[style_idx].0.end <= raw_off {
                    style_idx += 1;
                }
                let len = if ch == '\t' { 1 } else { disp_len };
                if let Some((r, st)) = styles.get(style_idx)
                    && r.start <= raw_off
                    && raw_off < r.end
                {
                    match out.last_mut() {
                        Some((last, last_st)) if last.end == seg_byte && last_st == st => {
                            last.end += len
                        }
                        _ => out.push((seg_byte..seg_byte + len, st.clone())),
                    }
                }
                seg_byte += len;
            }
            disp_char += 1;
        }
        if disp_char >= char_end {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gitti_test::test]
    fn every_mapped_language_compiles_and_highlights() {
        let samples = [
            ("rust", "// c\nfn main() { let x = \"s\"; }\n"),
            ("python", "# c\ndef f():\n    return 'x'\n"),
            ("javascript", "// c\nfunction f() { return 1; }\n"),
            ("typescript", "// c\nfunction f(): number { return 1; }\n"),
            ("tsx", "// c\nconst a = <div>x</div>;\n"),
            ("json", "{\n\"a\": 1\n}\n"),
            ("toml", "# c\n[a]\nb = \"c\"\n"),
            ("yaml", "# c\na: b\n"),
            ("c", "// c\nint main() { return 0; }\n"),
            ("cpp", "// c\nnamespace a { int f() { return 1; } }\n"),
            ("hlsl", "// c\nfloat4 main(float4 p : POSITION) : SV_Target { return p; }\n"),
            ("glsl", "// c\nuniform float t;\nvoid main() { gl_Position = vec4(t); }\n"),
            ("objc", "// c\n@interface A : NSObject\n@end\n"),
            ("wgsl", "// c\nfn main() -> f32 { let x = 1.0; return x; }\n"),
            ("xml", "<!-- c -->\n<a b=\"c\">d</a>\n"),
            ("powershell", "# c\nfunction F { Write-Host \"x\" }\n"),
            ("ini", "; c\n[core]\nx = 1\n"),
            ("csharp", "// c\nclass A { int F() { return 1; } }\n"),
            ("go", "// c\nfunc main() { return }\n"),
            ("java", "// c\nclass A { int f() { return 1; } }\n"),
            ("bash", "# c\nif true; then echo \"x\"; fi\n"),
            ("html", "<!-- c -->\n<p class=\"a\">x</p>\n"),
            ("css", "/* c */\na { color: red; }\n"),
            ("ruby", "# c\ndef f; 'x'; end\n"),
            ("lua", "-- c\nlocal x = 'y'\n"),
            ("zig", "// c\nconst x: u8 = 1;\n"),
            ("scala", "// c\nobject A { def f = 1 }\n"),
            ("sql", "-- c\nSELECT a FROM b;\n"),
            ("make", "# c\nall: a\n\techo x\n"),
            ("cmake", "# c\nset(A \"b\")\n"),
            ("swift", "// c\nfunc f() -> Int { return 1 }\n"),
            ("diff", "diff --git a/x b/x\n+added\n"),
        ];
        for (lang, text) in samples {
            let lines = highlight(lang, text).unwrap_or_else(|| panic!("{lang} not supported"));
            assert!(
                lines.iter().any(|l| !l.is_empty()),
                "{lang}: nothing highlighted"
            );
        }
        // Block comments cover every line they span.
        let lines = highlight("rust", "/* a\n b */\nfn main() {}\n").unwrap();
        assert_eq!(lines[0], vec![(0..4, Category::Comment)]);
        assert_eq!(lines[1], vec![(0..5, Category::Comment)]);
        assert!(lines[2].iter().any(|(_, c)| *c == Category::Keyword));
        assert_eq!(highlight("cobol", "x"), None);
    }

    #[gitti_test::test]
    fn languages_and_segments() {
        assert_eq!(language_for_path("src/main.rs"), Some("rust"));
        assert_eq!(language_for_path("a/CMakeLists.txt"), Some("cmake"));
        assert_eq!(language_for_path("include/foo.h"), Some("cpp"));
        assert_eq!(language_for_path("README"), None);
        // raw "\tfn x" with a style on "fn" (bytes 1..3) → displayed "    fn x": 4..6
        let styles = vec![(1..3, 7u8)];
        assert_eq!(segment_ranges("\tfn x", &styles, 0, usize::MAX)[0].0, 4..6);
        assert_eq!(segment_ranges("\tfn x", &styles, 5, usize::MAX)[0].0, 0..1);
    }
}
