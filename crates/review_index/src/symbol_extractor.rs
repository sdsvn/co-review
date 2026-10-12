//! Symbol extraction from source files via Tree-sitter.
//!
//! The `SymbolExtractor` trait allows swapping in Zed's language registry and outline queries later;
//! `DefaultSymbolExtractor` uses native tree-sitter grammars from crates.io with the same
//! node-kind-to-symbol mapping as the TypeScript `syntax-service-impl.ts`.

use std::collections::HashMap;
use std::sync::LazyLock;

use tree_sitter::{Language, Node, Parser};
use tree_sitter_language::LanguageFn;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The kind of a symbol, matching the TypeScript `SyntaxSymbolKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Struct,
    Interface,
    Enum,
    Module,
    Type,
    Function,
    Method,
    Constructor,
    Field,
}

/// A symbol extracted from source code, with optional children (members).
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub children: Vec<Symbol>,
}

/// Trait for extracting symbols from source code. The default implementation uses tree-sitter
/// grammars from crates.io; a later implementation can use Zed's language registry instead.
pub trait SymbolExtractor: Send + Sync {
    /// Extract top-level symbols from the given source text in the given language.
    fn extract(&self, language_id: &str, source: &[u8]) -> Vec<Symbol>;
}

// ---------------------------------------------------------------------------
// Grammar registry
// ---------------------------------------------------------------------------

/// Maps Monaco/VS Code language id to a grammar identifier used internally.
static GRAMMARS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("go", "go"),
        ("typescript", "typescript"),
        ("typescriptreact", "tsx"),
        ("javascript", "javascript"),
        ("javascriptreact", "javascript"),
        ("python", "python"),
        ("java", "java"),
        ("rust", "rust"),
        ("csharp", "c-sharp"),
        ("cpp", "cpp"),
        ("c", "c"),
        ("ruby", "ruby"),
        ("php", "php"),
        ("shellscript", "bash"),
        ("powershell", "powershell"),
    ])
});

/// Definition node types per grammar, and the symbol kind each one produces.
/// Matches the TypeScript `DEFINITIONS` exactly.
static DEFINITIONS: LazyLock<HashMap<&'static str, HashMap<&'static str, SymbolKind>>> =
    LazyLock::new(|| {
        use SymbolKind::*;
        HashMap::from([
            (
                "go",
                HashMap::from([
                    ("type_spec", Type),
                    ("function_declaration", Function),
                    ("method_declaration", Method),
                    ("method_elem", Method),
                    ("field_declaration", Field),
                ]),
            ),
            (
                "typescript",
                HashMap::from([
                    ("class_declaration", Class),
                    ("abstract_class_declaration", Class),
                    ("interface_declaration", Interface),
                    ("enum_declaration", Enum),
                    ("type_alias_declaration", Type),
                    ("function_declaration", Function),
                    ("generator_function_declaration", Function),
                    ("method_definition", Method),
                    ("method_signature", Method),
                    ("abstract_method_signature", Method),
                    ("internal_module", Module),
                    ("public_field_definition", Field),
                    ("variable_declarator", Function),
                ]),
            ),
            (
                "javascript",
                HashMap::from([
                    ("class_declaration", Class),
                    ("function_declaration", Function),
                    ("generator_function_declaration", Function),
                    ("method_definition", Method),
                    ("field_definition", Field),
                    ("variable_declarator", Function),
                ]),
            ),
            ("python", HashMap::from([("class_definition", Class), ("function_definition", Function)])),
            (
                "java",
                HashMap::from([
                    ("class_declaration", Class),
                    ("interface_declaration", Interface),
                    ("enum_declaration", Enum),
                    ("record_declaration", Class),
                    ("method_declaration", Method),
                    ("constructor_declaration", Constructor),
                ]),
            ),
            (
                "rust",
                HashMap::from([
                    ("function_item", Function),
                    ("function_signature_item", Function),
                    ("struct_item", Struct),
                    ("enum_item", Enum),
                    ("trait_item", Interface),
                    ("impl_item", Class),
                    ("mod_item", Module),
                    ("type_item", Type),
                ]),
            ),
            (
                "c-sharp",
                HashMap::from([
                    ("namespace_declaration", Module),
                    ("file_scoped_namespace_declaration", Module),
                    ("class_declaration", Class),
                    ("struct_declaration", Struct),
                    ("interface_declaration", Interface),
                    ("enum_declaration", Enum),
                    ("record_declaration", Class),
                    ("method_declaration", Method),
                    ("constructor_declaration", Constructor),
                    ("property_declaration", Field),
                ]),
            ),
            (
                "cpp",
                HashMap::from([
                    ("namespace_definition", Module),
                    ("class_specifier", Class),
                    ("struct_specifier", Struct),
                    ("enum_specifier", Enum),
                    ("function_definition", Function),
                ]),
            ),
            (
                "ruby",
                HashMap::from([
                    ("module", Module),
                    ("class", Class),
                    ("method", Method),
                    ("singleton_method", Method),
                ]),
            ),
            (
                "php",
                HashMap::from([
                    ("namespace_definition", Module),
                    ("class_declaration", Class),
                    ("interface_declaration", Interface),
                    ("trait_declaration", Interface),
                    ("enum_declaration", Enum),
                    ("function_definition", Function),
                    ("method_declaration", Method),
                ]),
            ),
            ("bash", HashMap::from([("function_definition", Function)])),
            (
                "powershell",
                HashMap::from([("function_statement", Function), ("class_statement", Class)]),
            ),
            // tsx shares typescript's definitions.
            (
                "tsx",
                HashMap::from([
                    ("class_declaration", Class),
                    ("abstract_class_declaration", Class),
                    ("interface_declaration", Interface),
                    ("enum_declaration", Enum),
                    ("type_alias_declaration", Type),
                    ("function_declaration", Function),
                    ("generator_function_declaration", Function),
                    ("method_definition", Method),
                    ("method_signature", Method),
                    ("abstract_method_signature", Method),
                    ("internal_module", Module),
                    ("public_field_definition", Field),
                    ("variable_declarator", Function),
                ]),
            ),
            // c shares cpp definitions (the TS maps both to the cpp grammar, but we use separate
            // grammars; the node types are the same).
            (
                "c",
                HashMap::from([
                    ("namespace_definition", Module),
                    ("class_specifier", Class),
                    ("struct_specifier", Struct),
                    ("enum_specifier", Enum),
                    ("function_definition", Function),
                ]),
            ),
        ])
    });

/// Returns the tree-sitter Language for a grammar id.
fn language_for_grammar(grammar: &str) -> Option<Language> {
    let lang_fn: LanguageFn = match grammar {
        "go" => tree_sitter_go::LANGUAGE,
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX,
        "javascript" => tree_sitter_javascript::LANGUAGE,
        "python" => tree_sitter_python::LANGUAGE,
        "java" => tree_sitter_java::LANGUAGE,
        "rust" => tree_sitter_rust::LANGUAGE,
        "c-sharp" => tree_sitter_c_sharp::LANGUAGE,
        "cpp" => tree_sitter_cpp::LANGUAGE,
        "c" => tree_sitter_c::LANGUAGE,
        "ruby" => tree_sitter_ruby::LANGUAGE,
        "php" => tree_sitter_php::LANGUAGE_PHP,
        "bash" => tree_sitter_bash::LANGUAGE,
        "powershell" => tree_sitter_powershell::LANGUAGE,
        _ => return None,
    };
    Some(lang_fn.into())
}

// ---------------------------------------------------------------------------
// Default extractor
// ---------------------------------------------------------------------------

/// The default symbol extractor using native tree-sitter grammars.
pub struct DefaultSymbolExtractor;

impl SymbolExtractor for DefaultSymbolExtractor {
    fn extract(&self, language_id: &str, source: &[u8]) -> Vec<Symbol> {
        let grammar = match GRAMMARS.get(language_id) {
            Some(g) => *g,
            None => return Vec::new(),
        };
        let language = match language_for_grammar(grammar) {
            Some(l) => l,
            None => return Vec::new(),
        };
        let definitions = match DEFINITIONS.get(grammar) {
            Some(d) => d,
            None => return Vec::new(),
        };

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        collect_symbols(grammar, &tree.root_node(), definitions, source)
    }
}

/// Recursively collect symbols from a tree-sitter node, matching the TS `collect` function.
fn collect_symbols(
    grammar: &str,
    node: &Node,
    definitions: &HashMap<&str, SymbolKind>,
    source: &[u8],
) -> Vec<Symbol> {
    let mut result = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = definitions.get(child.kind());
        let name_node = kind.and_then(|_| name_of(grammar, &child, source));

        if let (Some(&kind), Some(name_node)) = (kind, name_node) {
            if is_definition(grammar, &child) {
                let name = qualified_name(grammar, &child, &name_node, source);
                let children = collect_symbols(grammar, &child, definitions, source);
                result.push(Symbol {
                    name,
                    kind,
                    children,
                });
            } else {
                result.extend(collect_symbols(grammar, &child, definitions, source));
            }
        } else {
            result.extend(collect_symbols(grammar, &child, definitions, source));
        }
    }
    result
}

/// Filters definition node types that are only sometimes definitions.
/// Matches the TS `isDefinition` method.
fn is_definition(grammar: &str, node: &Node) -> bool {
    if node.kind() == "variable_declarator" {
        // Only `const foo = () => {}` / `const foo = function() {}` style functions.
        let value = node.child_by_field_name("value");
        return value
            .map(|v| {
                matches!(
                    v.kind(),
                    "arrow_function"
                        | "function_expression"
                        | "function"
                        | "generator_function"
                )
            })
            .unwrap_or(false);
    }
    if grammar == "go" && node.kind() == "field_declaration" {
        return false;
    }
    true
}

/// Find the name node for a definition. Matches the TS `nameOf` method.
fn name_of<'a>(grammar: &str, node: &Node<'a>, _source: &[u8]) -> Option<Node<'a>> {
    // Rust impl_item uses "type" field instead of "name".
    if grammar == "rust" && node.kind() == "impl_item" {
        return node.child_by_field_name("type");
    }
    let name = node.child_by_field_name("name");
    if name.is_some() {
        return name;
    }
    // C/C++ functions: dig through the declarator chain to the identifier.
    let mut declarator = node.child_by_field_name("declarator");
    while let Some(decl) = declarator {
        let kind = decl.kind();
        if matches!(
            kind,
            "identifier"
                | "field_identifier"
                | "qualified_identifier"
                | "destructor_name"
                | "operator_name"
        ) {
            return Some(decl);
        }
        declarator = decl.child_by_field_name("declarator").or_else(|| {
            let mut cursor = decl.walk();
            decl.named_children(&mut cursor)
                .find(|c| c.kind().ends_with("identifier"))
        });
    }
    None
}

/// Go methods: qualify with receiver type (`OrderService.CreateOrder`).
/// Matches the TS `qualifiedName` method.
fn qualified_name(grammar: &str, node: &Node, name_node: &Node, source: &[u8]) -> String {
    let name_text = name_node
        .utf8_text(source)
        .unwrap_or_default()
        .to_string();

    if grammar == "go"
        && node.kind() == "method_declaration"
        && let Some(receiver) = node.child_by_field_name("receiver")
    {
        let receiver_text = receiver.utf8_text(source).unwrap_or_default();
        if let Some(type_name) = extract_go_receiver_type(receiver_text) {
            return format!("{type_name}.{name_text}");
        }
    }
    name_text
}

/// Extract the receiver type name from a Go method receiver parameter text.
/// Handles patterns like `(s *Service)`, `(s Service)`, `(s Service[T])`, `(*Service)`.
fn extract_go_receiver_type(receiver_text: &str) -> Option<String> {
    // The TS uses: /([A-Za-z_]\w*)(\[[^\]]*\])?\s*\)$/
    let trimmed = receiver_text.trim();
    let trimmed = trimmed.strip_suffix(')')?;
    let trimmed = trimmed.trim_end();
    // Remove generic parameters if present.
    let trimmed = if let Some(bracket_pos) = trimmed.rfind('[') {
        &trimmed[..bracket_pos]
    } else {
        trimmed
    };
    let trimmed = trimmed.trim_end();
    // Find the last word (the type name), which may be preceded by *.
    let last_word_start = trimmed
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let type_name = &trimmed[last_word_start..];
    if type_name.is_empty()
        || !type_name
            .chars()
            .next()
            .map(|c| c.is_alphabetic() || c == '_')
            .unwrap_or(false)
    {
        return None;
    }
    Some(type_name.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_go_receiver_type() {
        assert_eq!(
            extract_go_receiver_type("(s *Service)"),
            Some("Service".to_string())
        );
        assert_eq!(
            extract_go_receiver_type("(s Service)"),
            Some("Service".to_string())
        );
        assert_eq!(
            extract_go_receiver_type("(s Service[T])"),
            Some("Service".to_string())
        );
        assert_eq!(
            extract_go_receiver_type("(*Service)"),
            Some("Service".to_string())
        );
    }

    #[test]
    fn test_extract_rust_symbols() {
        let source = br#"
pub fn hello() {}
pub struct Point { x: f64, y: f64 }
impl Point {
    pub fn new(x: f64, y: f64) -> Self { Self { x, y } }
    pub fn distance(&self) -> f64 { (self.x * self.x + self.y * self.y).sqrt() }
}
pub enum Color { Red, Green, Blue }
pub trait Drawable { fn draw(&self); }
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("rust", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"), "should find hello: {:?}", names);
        assert!(names.contains(&"Point"), "should find Point struct: {:?}", names);
        assert!(names.contains(&"Color"), "should find Color enum: {:?}", names);
        assert!(names.contains(&"Drawable"), "should find Drawable trait: {:?}", names);
        // impl Point should appear with children.
        let impl_point = symbols.iter().find(|s| s.name == "Point" && s.kind == SymbolKind::Class);
        assert!(impl_point.is_some(), "should find impl Point: {:?}", names);
        let children: Vec<&str> = impl_point.unwrap().children.iter().map(|c| c.name.as_str()).collect();
        assert!(children.contains(&"new"), "impl Point should have new: {:?}", children);
        assert!(children.contains(&"distance"), "impl Point should have distance: {:?}", children);
    }

    #[test]
    fn test_extract_go_symbols() {
        let source = br#"
package main

type OrderService struct {
    db *DB
}

func NewOrderService(db *DB) *OrderService {
    return &OrderService{db: db}
}

func (s *OrderService) CreateOrder(ctx context.Context) error {
    return nil
}
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("go", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"OrderService"), "should find OrderService: {:?}", names);
        assert!(names.contains(&"NewOrderService"), "should find NewOrderService: {:?}", names);
        assert!(names.contains(&"OrderService.CreateOrder"), "should find OrderService.CreateOrder: {:?}", names);
    }

    #[test]
    fn test_extract_typescript_symbols() {
        let source = br#"
export class UserService {
    constructor(private db: Database) {}
    async getUser(id: string): Promise<User> { return {} as User; }
    private readonly cache = new Map();
}
export function helper() {}
export const handler = () => {};
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("typescript", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"UserService"), "should find UserService: {:?}", names);
        assert!(names.contains(&"helper"), "should find helper: {:?}", names);
        assert!(names.contains(&"handler"), "should find handler: {:?}", names);
    }

    #[test]
    fn test_extract_python_symbols() {
        let source = br#"
class Animal:
    def __init__(self, name):
        self.name = name
    def speak(self):
        pass

def standalone():
    pass
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("python", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Animal"), "should find Animal: {:?}", names);
        assert!(names.contains(&"standalone"), "should find standalone: {:?}", names);
        let animal = symbols.iter().find(|s| s.name == "Animal").expect("Animal");
        let methods: Vec<&str> = animal.children.iter().map(|c| c.name.as_str()).collect();
        assert!(methods.contains(&"__init__"), "Animal should have __init__: {:?}", methods);
        assert!(methods.contains(&"speak"), "Animal should have speak: {:?}", methods);
    }

    #[test]
    fn test_extract_java_symbols() {
        let source = br#"
public class Calculator {
    public Calculator() {}
    public int add(int a, int b) { return a + b; }
}
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("java", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Calculator"), "should find Calculator: {:?}", names);
    }

    #[test]
    fn test_extract_c_symbols() {
        let source = br#"
struct Point { int x; int y; };
int add(int a, int b) { return a + b; }
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("c", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "should find add: {:?}", names);
    }

    #[test]
    fn test_extract_cpp_symbols() {
        let source = br#"
namespace math {
class Vector {
};
}
int main() { return 0; }
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("cpp", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"math"), "should find math namespace: {:?}", names);
        assert!(names.contains(&"main"), "should find main: {:?}", names);
    }

    #[test]
    fn test_variable_declarator_filter() {
        // Only arrow functions / function expressions count as definitions.
        let source = br#"
const handler = () => {};
const value = 42;
const fn2 = function() {};
"#;
        let extractor = DefaultSymbolExtractor;
        let symbols = extractor.extract("javascript", source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"handler"), "arrow function should be a symbol: {:?}", names);
        assert!(names.contains(&"fn2"), "function expression should be a symbol: {:?}", names);
        assert!(!names.contains(&"value"), "plain const should not be a symbol: {:?}", names);
    }
}
