//! Symbol extraction from source files via Tree-sitter.
//!
//! The `SymbolExtractor` trait allows swapping in Zed's language registry and outline queries later;
//! `DefaultSymbolExtractor` uses native tree-sitter grammars from crates.io with the same
//! node-kind-to-symbol mapping as the TypeScript `syntax-service-impl.ts`.

use tree_sitter::{Language, Node, Parser};

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

/// The grammar for a Monaco/VS Code language id: the name its definitions and quirks go by, and
/// the parser. TSX shares TypeScript's definitions and C shares C++'s, as in the TypeScript.
fn grammar_of(language_id: &str) -> Option<(&'static str, Language)> {
    Some(match language_id {
        "go" => ("go", tree_sitter_go::LANGUAGE.into()),
        "typescript" => ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "typescriptreact" => ("typescript", tree_sitter_typescript::LANGUAGE_TSX.into()),
        "javascript" | "javascriptreact" => ("javascript", tree_sitter_javascript::LANGUAGE.into()),
        "python" => ("python", tree_sitter_python::LANGUAGE.into()),
        "java" => ("java", tree_sitter_java::LANGUAGE.into()),
        "rust" => ("rust", tree_sitter_rust::LANGUAGE.into()),
        "csharp" => ("c-sharp", tree_sitter_c_sharp::LANGUAGE.into()),
        "cpp" => ("cpp", tree_sitter_cpp::LANGUAGE.into()),
        "c" => ("cpp", tree_sitter_c::LANGUAGE.into()),
        "ruby" => ("ruby", tree_sitter_ruby::LANGUAGE.into()),
        "php" => ("php", tree_sitter_php::LANGUAGE_PHP.into()),
        "shellscript" => ("bash", tree_sitter_bash::LANGUAGE.into()),
        "powershell" => ("powershell", tree_sitter_powershell::LANGUAGE.into()),
        _ => return None,
    })
}

/// Definition node types per grammar, and the symbol kind each one produces (the TypeScript `DEFINITIONS`).
fn definitions(grammar: &str) -> &'static [(&'static str, SymbolKind)] {
    use SymbolKind::*;
    match grammar {
        "go" => &[
            ("type_spec", Type),
            ("function_declaration", Function),
            ("method_declaration", Method),
            ("method_elem", Method),
            ("field_declaration", Field),
        ],
        "typescript" => &[
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
        ],
        "javascript" => &[
            ("class_declaration", Class),
            ("function_declaration", Function),
            ("generator_function_declaration", Function),
            ("method_definition", Method),
            ("field_definition", Field),
            ("variable_declarator", Function),
        ],
        "python" => &[("class_definition", Class), ("function_definition", Function)],
        "java" => &[
            ("class_declaration", Class),
            ("interface_declaration", Interface),
            ("enum_declaration", Enum),
            ("record_declaration", Class),
            ("method_declaration", Method),
            ("constructor_declaration", Constructor),
        ],
        "rust" => &[
            ("function_item", Function),
            ("function_signature_item", Function),
            ("struct_item", Struct),
            ("enum_item", Enum),
            ("trait_item", Interface),
            ("impl_item", Class),
            ("mod_item", Module),
            ("type_item", Type),
        ],
        "c-sharp" => &[
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
        ],
        "cpp" => &[
            ("namespace_definition", Module),
            ("class_specifier", Class),
            ("struct_specifier", Struct),
            ("enum_specifier", Enum),
            ("function_definition", Function),
        ],
        "ruby" => &[
            ("module", Module),
            ("class", Class),
            ("method", Method),
            ("singleton_method", Method),
        ],
        "php" => &[
            ("namespace_definition", Module),
            ("class_declaration", Class),
            ("interface_declaration", Interface),
            ("trait_declaration", Interface),
            ("enum_declaration", Enum),
            ("function_definition", Function),
            ("method_declaration", Method),
        ],
        "bash" => &[("function_definition", Function)],
        "powershell" => &[("function_statement", Function), ("class_statement", Class)],
        _ => &[],
    }
}

/// The default symbol extractor using native tree-sitter grammars.
#[derive(Default)]
pub struct DefaultSymbolExtractor;

impl SymbolExtractor for DefaultSymbolExtractor {
    fn extract(&self, language_id: &str, source: &[u8]) -> Vec<Symbol> {
        let Some((grammar, language)) = grammar_of(language_id) else {
            return Vec::new();
        };
        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(source, None) else {
            return Vec::new();
        };
        collect_symbols(grammar, &tree.root_node(), definitions(grammar), source)
    }
}

/// Recursively collect symbols from a tree-sitter node, matching the TS `collect` function.
fn collect_symbols(
    grammar: &str,
    node: &Node,
    definitions: &[(&str, SymbolKind)],
    source: &[u8],
) -> Vec<Symbol> {
    let mut result = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let definition = definitions
            .iter()
            .find(|(kind, _)| *kind == child.kind())
            .and_then(|&(_, kind)| Some((kind, name_of(grammar, &child)?)))
            .filter(|_| is_definition(grammar, &child));
        let children = collect_symbols(grammar, &child, definitions, source);
        match definition {
            Some((kind, name_node)) => result.push(Symbol {
                name: qualified_name(grammar, &child, &name_node, source),
                kind,
                children,
            }),
            None => result.extend(children),
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
fn name_of<'a>(grammar: &str, node: &Node<'a>) -> Option<Node<'a>> {
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
