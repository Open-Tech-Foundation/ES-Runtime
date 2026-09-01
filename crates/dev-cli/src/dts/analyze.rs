//! One `.d.ts`, read into the shape a linker can work with.
//!
//! The output of this module is deliberately **not** an AST. A declaration is
//! carried as the *text* that produced it, plus the byte ranges inside that text
//! where a module-scope name appears. Everything the linker does — inlining,
//! renaming, dropping — is then a string operation on those ranges.
//!
//! That is the central design choice here, and it is worth the paragraph. The
//! obvious alternative is to move AST nodes between modules and print the
//! result, which is what a JavaScript bundler does. For declarations it is the
//! wrong trade:
//!
//! * **JSDoc survives byte for byte.** A published `.d.ts` is read by humans and
//!   by editors, and its comments *are* the documentation — hovering a symbol in
//!   an editor shows them. Reprinting an AST reflows or drops them; oxc's own
//!   printer already reindents the JSDoc it keeps.
//! * **No arena crosses a module boundary.** Every AST here is parsed, reduced
//!   to owned text, and dropped, so nothing is borrowed from a parser that has
//!   to be kept alive for the whole build.
//! * **A rename is auditable.** It is a list of byte ranges and the name at
//!   each; a test can assert exactly which bytes moved.
//!
//! The risk it accepts is that a *missed* reference site becomes a dangling name
//! after a rename. That is why the sites come from `oxc`'s semantic analysis —
//! resolved references to a module-scope binding, which is the same information
//! a renaming refactor in an editor uses — rather than from a hand-written walk
//! over the type syntax, which would silently miss the corner of TypeScript
//! nobody thought of.

use std::ops::Range;

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier, ModuleExportName,
    Statement,
};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType, Span};

/// A `.d.ts` reduced to declarations, what it imports, and what it exports.
#[derive(Debug, Default)]
pub struct Analysis {
    /// Top-level declarations, in source order.
    pub decls: Vec<Decl>,
    /// Every name the module brings in from elsewhere.
    pub imports: Vec<Import>,
    /// `export { … }`, `export { … } from`, and the `export` modifier on a
    /// declaration — all reduced to the same entry.
    pub exports: Vec<Export>,
    /// The specifiers of `export * from …`, in source order. Order matters:
    /// TypeScript resolves an ambiguity between two of them by excluding the
    /// name, and an explicit export beats both.
    pub star_exports: Vec<String>,
}

/// One top-level declaration.
#[derive(Debug)]
pub struct Decl {
    /// The names it binds. Usually one; `declare const a: A, b: B` binds two.
    pub names: Vec<String>,
    /// Its text, with any `export` modifier removed and its leading comments
    /// kept. This is what gets written into the bundle.
    pub text: String,
    /// Every place in `text` naming a top-level binding — the declaration's own
    /// name included, since that is renamed too.
    pub sites: Vec<Site>,
}

/// A place in a declaration's text where a top-level name appears.
#[derive(Debug, Clone)]
pub struct Site {
    pub range: Range<usize>,
    pub name: String,
}

/// A name an import binds at the top level of a module.
///
/// A name a *declaration* binds is not here: the declaration already carries
/// its own names, and a second list of them would be a second thing to keep in
/// step.
#[derive(Debug)]
pub struct Import {
    /// The name as used in this module, which is what a site says.
    pub local: String,
    pub specifier: String,
    pub imported: Imported,
    /// Whether it arrived through `import type` — on the declaration or on the
    /// specifier. Kept so the bundle imports a type the way the source did
    /// rather than widening it to a value import.
    pub type_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Imported {
    Named(String),
    Default,
}

/// One name a module makes available to importers.
#[derive(Debug)]
pub struct Export {
    /// The name an importer asks for. `default` for a default export.
    pub exported: String,
    /// Where it comes from: a binding in this module, or another module.
    pub from: ExportSource,
}

#[derive(Debug)]
pub enum ExportSource {
    /// A top-level binding of this module, by local name.
    Local(String),
    /// `export { imported as exported } from specifier` — never bound locally,
    /// so it cannot be looked up among this module's bindings.
    Reexport { specifier: String, imported: String },
}

/// Reads `source` — the text of a `.d.ts` — into an [`Analysis`].
///
/// `label` names the file in any error, and is the only thing this needs a path
/// for: nothing here touches the filesystem.
pub fn analyze(label: &str, source: &str) -> Result<Analysis, String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::d_ts()).parse();
    if let Some(error) = parsed
        .diagnostics
        .iter()
        .find(|d| d.severity == oxc::diagnostics::Severity::Error)
    {
        return Err(format!("{label}: {error}"));
    }
    let program = &parsed.program;
    // `with_build_nodes` is off by default, and without it `Semantic::nodes` is
    // empty — which is exactly what a reference has to be looked up in to find
    // the span it occupies. A reference carries a `NodeId`, not a position.
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(program)
        .semantic;

    // Every reference site in the file, sorted by position, so slicing a
    // declaration's range out of it is a single pass.
    let sites = reference_sites(&semantic);

    if let Some(specifier) = local_import_type(program) {
        return Err(unsupported(
            label,
            &format!("import(\"{specifier}\")"),
            "an inline import type is a reference to another module of this library, \
             and linking resolves import *statements* — so the one file would keep a \
             relative specifier with nothing beside it to resolve to",
        ));
    }

    let mut analysis = Analysis::default();
    for statement in &program.body {
        read_statement(label, source, program, &sites, statement, &mut analysis)?;
    }
    Ok(analysis)
}

/// The first `import("./x")` **type** naming a module of this library, if there
/// is one.
///
/// It is refused rather than emitted, which is this module's rule everywhere
/// (see its header): the alternative is a single `index.d.ts` carrying
/// `import("./DateTime.js")` with no `DateTime.d.ts` beside it any more —
/// TS2307 in the consumer's editor, from a build that said it succeeded.
///
/// A **bare** specifier is left alone: `import("hono").Context` names a package
/// the consumer resolves for themselves, exactly as an import statement of it
/// would, and the linker keeps those.
///
/// A visitor, because an import type is a type: it sits wherever a type may sit
/// — a property, a union, a type argument several levels down — and not at the
/// top of the file where the import statements are.
fn local_import_type(program: &oxc::ast::ast::Program<'_>) -> Option<String> {
    use oxc::ast_visit::Visit;

    #[derive(Default)]
    struct Found(Option<String>);

    impl<'a> Visit<'a> for Found {
        fn visit_ts_import_type(&mut self, it: &oxc::ast::ast::TSImportType<'a>) {
            let specifier = it.source.value.as_str();
            if self.0.is_none() && (specifier.starts_with("./") || specifier.starts_with("../")) {
                self.0 = Some(specifier.to_string());
            }
            oxc::ast_visit::walk::walk_ts_import_type(self, it);
        }
    }

    let mut found = Found::default();
    found.visit_program(program);
    found.0
}

/// Every place a top-level binding is named, as absolute spans in the source.
///
/// Both halves matter and neither is optional: a binding's **own** identifier
/// (renaming `Foo` means rewriting `interface Foo` too) and every **resolved
/// reference** to it. Taking them from semantic analysis rather than a syntax
/// walk is what makes this correct for type positions nobody enumerated —
/// `extends`, `implements`, a conditional type's `infer`, a mapped type's
/// constraint, a default type argument.
fn reference_sites(semantic: &oxc::semantic::Semantic<'_>) -> Vec<(Span, String)> {
    let scoping = semantic.scoping();
    let mut sites = Vec::new();
    for symbol_id in scoping.symbol_ids() {
        // Top level only. A name inside a function body or a type parameter
        // list is scoped to it and can never collide across modules, so
        // renaming it would be wrong as well as unnecessary.
        if scoping.symbol_scope_id(symbol_id) != scoping.root_scope_id() {
            continue;
        }
        let name = scoping.symbol_name(symbol_id).to_string();
        sites.push((scoping.symbol_span(symbol_id), name.clone()));
        for reference in scoping.get_resolved_references(symbol_id) {
            let node = semantic.nodes().get_node(reference.node_id());
            sites.push((node.kind().span(), name.clone()));
        }
    }
    sites.sort_by_key(|(span, _)| (span.start, span.end));
    sites
}

/// The sites falling inside `span`, rebased onto a text that starts with
/// `prefix_len` bytes of leading comments.
fn sites_within(sites: &[(Span, String)], span: Span, prefix_len: usize) -> Vec<Site> {
    sites
        .iter()
        .filter(|(site, _)| site.start >= span.start && site.end <= span.end)
        .map(|(site, name)| Site {
            range: (site.start - span.start) as usize + prefix_len
                ..(site.end - span.start) as usize + prefix_len,
            name: name.clone(),
        })
        .collect()
}

/// The leading comments attached to the token at `start`, as text.
///
/// `attached_to` is the parser's own answer to "which token does this comment
/// document", which is more reliable than scanning backwards over whitespace and
/// guessing.
fn leading_comments(source: &str, program: &oxc::ast::ast::Program<'_>, start: u32) -> String {
    let mut text = String::new();
    for comment in &program.comments {
        if comment.attached_to == start
            && comment.position == oxc::ast::ast::CommentPosition::Leading
        {
            text.push_str(&source[comment.span.start as usize..comment.span.end as usize]);
            text.push('\n');
        }
    }
    text
}

/// Records one top-level statement into `analysis`.
fn read_statement(
    label: &str,
    source: &str,
    program: &oxc::ast::ast::Program<'_>,
    sites: &[(Span, String)],
    statement: &Statement<'_>,
    analysis: &mut Analysis,
) -> Result<(), String> {
    match statement {
        Statement::ImportDeclaration(import) => {
            let specifier = import.source.value.to_string();
            let declaration_type_only = import.import_kind.is_type();
            let Some(specifiers) = &import.specifiers else {
                // `import "./side-effect.js"` — no bindings, and a declaration
                // file has no side effects to preserve.
                return Ok(());
            };
            for entry in specifiers {
                match entry {
                    ImportDeclarationSpecifier::ImportSpecifier(named) => {
                        let imported = match &named.imported {
                            ModuleExportName::IdentifierName(id) => id.name.to_string(),
                            ModuleExportName::IdentifierReference(id) => id.name.to_string(),
                            ModuleExportName::StringLiteral(s) => s.value.to_string(),
                        };
                        analysis.imports.push(Import {
                            local: named.local.name.to_string(),
                            specifier: specifier.clone(),
                            imported: Imported::Named(imported),
                            type_only: declaration_type_only || named.import_kind.is_type(),
                        });
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => {
                        analysis.imports.push(Import {
                            local: default.local.name.to_string(),
                            specifier: specifier.clone(),
                            imported: Imported::Default,
                            type_only: declaration_type_only,
                        });
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(ns) => {
                        return Err(unsupported(
                            label,
                            &format!("import * as {}", ns.local.name),
                            "inlining a namespace import means synthesising a namespace \
                             declaration to hold the other module's exports",
                        ));
                    }
                }
            }
            Ok(())
        }
        // `export interface Foo {}` — a declaration wearing an export modifier.
        Statement::ExportDeclaration(export) => {
            let index = push_decl(
                label,
                source,
                program,
                sites,
                statement.span(),
                &export.declaration,
                analysis,
            )?;
            for name in analysis.decls[index].names.clone() {
                analysis.exports.push(Export {
                    exported: name.clone(),
                    from: ExportSource::Local(name),
                });
            }
            Ok(())
        }
        // `export { Foo, Bar as Baz };`
        Statement::ExportNamedDeclaration(export) => {
            for entry in &export.specifiers {
                analysis.exports.push(Export {
                    exported: module_export_name(&entry.exported),
                    from: ExportSource::Local(module_export_name(&entry.local)),
                });
            }
            Ok(())
        }
        // `export { Foo } from "./foo.js";` — never bound locally, so it is
        // resolved against the other module rather than among our bindings.
        Statement::ExportFromDeclaration(export) => {
            let specifier = export.source.value.to_string();
            for entry in &export.specifiers {
                analysis.exports.push(Export {
                    exported: module_export_name(&entry.exported),
                    from: ExportSource::Reexport {
                        specifier: specifier.clone(),
                        imported: module_export_name(&entry.local),
                    },
                });
            }
            Ok(())
        }
        Statement::ExportAllDeclaration(export) => {
            if let Some(exported) = &export.exported {
                return Err(unsupported(
                    label,
                    &format!("export * as {} from …", module_export_name(exported)),
                    "a namespace re-export means synthesising a namespace declaration",
                ));
            }
            analysis.star_exports.push(export.source.value.to_string());
            Ok(())
        }
        Statement::ExportDefaultDeclaration(export) => {
            match &export.declaration {
                // `export default make;` — the common shape, and the one our own
                // generator emits for `export default function make() {}`.
                ExportDefaultDeclarationKind::Identifier(id) => {
                    analysis.exports.push(Export {
                        exported: "default".to_string(),
                        from: ExportSource::Local(id.name.to_string()),
                    });
                    Ok(())
                }
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                    let Some(id) = &function.id else {
                        return Err(unsupported(
                            label,
                            "export default function (…)",
                            "an anonymous default export has no name to refer to it by \
                             once it is one declaration among many",
                        ));
                    };
                    let name = id.name.to_string();
                    push_named_decl(
                        source,
                        program,
                        sites,
                        Inlined {
                            statement: statement.span(),
                            declaration: function.span(),
                            // **The one place a modifier has to be added rather
                            // than dropped.** `export default function f(): void;`
                            // carries no `declare` — the export modifier is what
                            // made it a declaration — so inlining it as
                            // `function f(): void;` is TS1046, a top-level
                            // declaration in a `.d.ts` with neither modifier.
                            // Every other shape here arrives as `export declare
                            // …` and keeps its `declare` when the `export` goes.
                            prefix: DECLARE,
                        },
                        vec![name.clone()],
                        analysis,
                    );
                    analysis.exports.push(Export {
                        exported: "default".to_string(),
                        from: ExportSource::Local(name),
                    });
                    Ok(())
                }
                ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                    let Some(id) = &class.id else {
                        return Err(unsupported(
                            label,
                            "export default class",
                            "an anonymous default export has no name to refer to it by \
                             once it is one declaration among many",
                        ));
                    };
                    let name = id.name.to_string();
                    push_named_decl(
                        source,
                        program,
                        sites,
                        Inlined {
                            statement: statement.span(),
                            declaration: class.span(),
                            prefix: DECLARE,
                        },
                        vec![name.clone()],
                        analysis,
                    );
                    analysis.exports.push(Export {
                        exported: "default".to_string(),
                        from: ExportSource::Local(name),
                    });
                    Ok(())
                }
                _ => Err(unsupported(
                    label,
                    "export default <expression>",
                    "only a default export naming a declaration can be inlined",
                )),
            }
        }
        Statement::TSExportAssignment(_) => Err(unsupported(
            label,
            "export =",
            "it is CommonJS, and this runtime has no module system but ES modules (D22)",
        )),
        // Anything else at the top level is either a declaration or not our
        // problem. A `.d.ts` has no executable statements.
        _ => {
            if let Some(declaration) = statement.as_declaration() {
                push_decl(
                    label,
                    source,
                    program,
                    sites,
                    statement.span(),
                    declaration,
                    analysis,
                )?;
            }
            Ok(())
        }
    }
}

/// Records a declaration and returns its index.
fn push_decl(
    label: &str,
    source: &str,
    program: &oxc::ast::ast::Program<'_>,
    sites: &[(Span, String)],
    statement_span: Span,
    declaration: &Declaration<'_>,
    analysis: &mut Analysis,
) -> Result<usize, String> {
    let names = declared_names(label, declaration)?;
    Ok(push_named_decl(
        source,
        program,
        sites,
        Inlined {
            statement: statement_span,
            declaration: declaration.span(),
            // Nothing to add: a named export's declaration already carries
            // `declare` where the grammar wants one, since that is how
            // isolated-declarations printed it.
            prefix: "",
        },
        names,
        analysis,
    ))
}

/// Records a declaration whose names are already known.
///
/// The text comes from the *declaration's* span rather than the statement's,
/// which is how the `export` modifier is dropped: `export interface Foo {}` and
/// `interface Foo {}` differ by exactly the bytes outside the inner span. The
/// comments come from the *statement's* start, because that is the token they
/// were attached to.
fn push_named_decl(
    source: &str,
    program: &oxc::ast::ast::Program<'_>,
    sites: &[(Span, String)],
    inlined: Inlined<'_>,
    names: Vec<String>,
    analysis: &mut Analysis,
) -> usize {
    let Inlined {
        statement,
        declaration,
        prefix,
    } = inlined;
    let doc = leading_comments(source, program, statement.start);
    let body = &source[declaration.start as usize..declaration.end as usize];
    let sites = sites_within(sites, declaration, doc.len() + prefix.len());
    analysis.decls.push(Decl {
        names,
        text: format!("{doc}{prefix}{body}"),
        sites,
    });
    analysis.decls.len() - 1
}

/// Where one declaration's text comes from: the statement it was written as
/// (which is what its comments are attached to), the declaration inside it
/// (which is the text, with the `export` modifier outside the span and so
/// dropped), and whatever has to be put back in front of it.
struct Inlined<'a> {
    statement: Span,
    declaration: Span,
    prefix: &'a str,
}

/// The top-level names a declaration binds.
fn declared_names(label: &str, declaration: &Declaration<'_>) -> Result<Vec<String>, String> {
    Ok(match declaration {
        Declaration::VariableDeclaration(variable) => variable
            .declarations
            .iter()
            .filter_map(|d| d.id.get_identifier_name().map(|n| n.to_string()))
            .collect(),
        Declaration::FunctionDeclaration(function) => function
            .id
            .as_ref()
            .map(|id| vec![id.name.to_string()])
            .unwrap_or_default(),
        Declaration::ClassDeclaration(class) => class
            .id
            .as_ref()
            .map(|id| vec![id.name.to_string()])
            .unwrap_or_default(),
        Declaration::TSTypeAliasDeclaration(alias) => vec![alias.id.name.to_string()],
        Declaration::TSInterfaceDeclaration(interface) => vec![interface.id.name.to_string()],
        Declaration::TSEnumDeclaration(enumeration) => vec![enumeration.id.name.to_string()],
        Declaration::TSNamespaceDeclaration(namespace) => vec![namespace.id.name.to_string()],
        Declaration::TSImportEqualsDeclaration(import) => vec![import.id.name.to_string()],
        // Both reach outside the module to change a type somewhere else, so
        // neither means the same thing once it is one declaration among many
        // in a file that declares no module.
        Declaration::TSExternalModuleDeclaration(module) => {
            return Err(unsupported(
                label,
                &format!("declare module {}", module.id.value),
                "a module augmentation belongs to the module it names, not to a bundle",
            ));
        }
        Declaration::TSGlobalDeclaration(_) => {
            return Err(unsupported(
                label,
                "declare global",
                "a global augmentation belongs to the module it was written in",
            ));
        }
    })
}

fn module_export_name(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(id) => id.name.to_string(),
        ModuleExportName::IdentifierReference(id) => id.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.to_string(),
    }
}

/// The refusal for a construct this bundler will not guess at.
///
/// Loud rather than silent, for the same reason a declaration that cannot be
/// derived fails the build: a `.d.ts` is *believed*, so producing a wrong one is
/// worse than producing none.
/// What a default-exported declaration is missing once its `export default` is
/// gone. See the call site.
const DECLARE: &str = "declare ";

fn unsupported(label: &str, construct: &str, why: &str) -> String {
    format!(
        "{label}: `{construct}` cannot be bundled into one declaration file — {why}.\n\n\
         Build without the declaration bundle to get one .d.ts per module, where \
         the construct \
         stands as written."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzed(source: &str) -> Analysis {
        analyze("test.d.ts", source).expect("analysis")
    }

    #[test]
    fn a_declaration_keeps_its_text_and_loses_its_export_modifier() {
        let analysis = analyzed("export interface Foo {\n\tx: number;\n}\n");
        assert_eq!(analysis.decls.len(), 1);
        assert_eq!(analysis.decls[0].names, ["Foo"]);
        assert!(
            analysis.decls[0].text.starts_with("interface Foo"),
            "{}",
            analysis.decls[0].text
        );
        assert_eq!(analysis.exports.len(), 1);
        assert_eq!(analysis.exports[0].exported, "Foo");
    }

    /// The reason this carries text rather than an AST: a published `.d.ts` is
    /// read by humans and by editors, and its JSDoc is the documentation.
    #[test]
    fn jsdoc_travels_with_the_declaration_byte_for_byte() {
        let source = "/**\n * Two lines,\n * and an  odd   space.\n */\nexport type Id = string;\n";
        let analysis = analyzed(source);
        let text = &analysis.decls[0].text;
        assert!(
            text.contains("/**\n * Two lines,\n * and an  odd   space.\n */"),
            "{text}"
        );
        assert!(text.contains("type Id = string"), "{text}");
    }

    /// The property everything else rests on. If a site is missed, a rename
    /// leaves a dangling name in a file nobody type-checks.
    #[test]
    fn a_type_reference_is_a_site_even_in_a_position_nobody_enumerated() {
        let analysis = analyzed(
            "interface Base {\n\tb: number;\n}\n\
             export interface Derived extends Base {\n\tmapped: { [K in keyof Base]: Base };\n}\n\
             export type Cond<T> = T extends Base ? Base : never;\n",
        );
        let mentions =
            |decl: &Decl, name: &str| decl.sites.iter().filter(|s| s.name == name).count();
        // `Base` in extends, in a mapped type's `keyof`, and as a value type.
        let derived = &analysis.decls[1];
        assert!(mentions(derived, "Base") >= 3, "{:?}", derived.sites);
        // …and in both branches of a conditional type.
        let conditional = &analysis.decls[2];
        assert!(
            mentions(conditional, "Base") >= 2,
            "{:?}",
            conditional.sites
        );
    }

    /// A site's range has to be exact: it is used to splice bytes.
    #[test]
    fn a_site_range_lands_on_the_name_it_claims() {
        let analysis = analyzed("interface Base {}\nexport type Alias = Base;\n");
        for decl in &analysis.decls {
            for site in &decl.sites {
                assert_eq!(&decl.text[site.range.clone()], site.name, "{:?}", decl.text);
            }
        }
    }

    #[test]
    fn imports_become_bindings_and_reexports_stay_unbound() {
        let analysis = analyzed(
            "import type { Dep } from \"./dep.js\";\n\
             export { type Other } from \"./other.js\";\n\
             export * from \"./star.js\";\n\
             export type Uses = Dep;\n",
        );
        assert_eq!(analysis.imports.len(), 1);
        assert_eq!(analysis.imports[0].local, "Dep");
        assert_eq!(analysis.star_exports, ["./star.js"]);
        let reexport = analysis
            .exports
            .iter()
            .find(|e| e.exported == "Other")
            .expect("re-export");
        assert!(matches!(reexport.from, ExportSource::Reexport { .. }));
    }

    #[test]
    fn a_renamed_export_keeps_both_names() {
        let analysis = analyzed("interface Internal {}\nexport { Internal as Public };\n");
        let export = &analysis.exports[0];
        assert_eq!(export.exported, "Public");
        assert!(matches!(&export.from, ExportSource::Local(name) if name == "Internal"));
    }

    #[test]
    fn a_default_export_names_the_declaration_it_points_at() {
        let analysis = analyzed("declare function make(): void;\nexport default make;\n");
        let export = &analysis.exports[0];
        assert_eq!(export.exported, "default");
        assert!(matches!(&export.from, ExportSource::Local(name) if name == "make"));
    }

    /// Refusals, which are the honest half of an MVP: each of these would need
    /// a synthesised namespace, and a wrong `.d.ts` is worse than none.
    #[test]
    fn the_constructs_it_cannot_bundle_are_refused_by_name() {
        for (source, needle) in [
            ("import * as ns from \"./x.js\";", "import * as ns"),
            ("export * as ns from \"./x.js\";", "export * as ns"),
            ("declare const x: number;\nexport = x;", "export ="),
        ] {
            let err = analyze("test.d.ts", source).expect_err(source);
            assert!(err.contains(needle), "{err}");
            // Surface-neutral: the same refusal reaches a reader who wrote
            // `--dts-bundle` and one who wrote `"dts-bundle": true`.
            assert!(err.contains("declaration bundle"), "{err}");
        }
    }
}
