//! TypeScript and JSX, stripped to JavaScript before the engine sees it.
//!
//! `esrun` runs JavaScript and nothing else — that is D22's "ES module packages
//! only", and it is why a deployed artifact is exactly the text that was
//! reviewed. But a developer writes `.ts` and `.tsx`, so something has to do the
//! stripping, and the only safe place for it is the machine they are working on.
//! This is that something.
//!
//! **Types are erased, never checked.** The same choice Node's
//! `--experimental-strip-types` and Bun make: a type error is your editor's job
//! and `tsc --noEmit`'s job, and doing it here would put a typechecker on the
//! critical path of every run for a diagnostic you have already seen. What
//! arrives at the engine is the same program with the annotations removed.
//!
//! **This transform rewrites no specifiers.** Resolution is the loader's
//! contract (D21/D40), and a transform that rewrote an import would be deciding
//! what a program means from inside the step that is only supposed to erase
//! types. Where `esdev` is wider than `esrun` — extensionless imports, a
//! directory's index, `./x.js` meaning `x.ts` — that widening lives in the
//! loader `esdev` installs and nowhere else, so this file's output is the same
//! program with the annotations removed and not a byte more
//! ([`es_runtime_cli_common::run::BundlerStyleLoader`]).

use std::path::Path;

use es_runtime_cli_common::run::SourceTransform;
use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer};

/// Strips TypeScript types and compiles JSX, leaving everything else alone.
///
/// It may also carry a **prelude** for one named module — the setup files
/// `esdev test --setup` was given. See [`TypeStripper::before`].
#[derive(Default)]
pub struct TypeStripper {
    /// The module the prelude belongs to, as a `file:` URL, and the specifiers
    /// to import ahead of it.
    prelude: Option<(String, Vec<String>)>,
    /// How JSX compiles. The project's `jsx` section, which a file's own pragma
    /// comments may still override.
    jsx: JsxSettings,
}

/// Where the function a JSX element compiles into comes from.
///
/// There are two answers and no third, and neither of them is a framework:
/// either the compiler writes the import, or it calls what the module already
/// has. Any library works as either — a package that exports `jsx`/`jsxs` from
/// a `jsx-runtime` subpath, or a function you import yourself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsxFunction {
    /// `<div/>` becomes `jsx("div", …)`, imported by the compiler from
    /// `<source>/jsx-runtime`.
    Imported { source: String },
    /// `<div/>` becomes `factory("div", …)`, resolved in the module's own
    /// scope: nothing is imported, so the module imports it itself.
    InScope {
        factory: String,
        fragment: Option<String>,
    },
}

/// How JSX compiles, for a project or for one file.
///
/// `function` is `None` until something says: a project has no default because
/// choosing one would be choosing a framework. A file that contains JSX and has
/// no answer is an error naming the file, not a guess.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsxSettings {
    /// Where the element function comes from, or `None` if nothing has said.
    pub function: Option<JsxFunction>,
    /// `__source` and `__self` on every element, which a dev-only renderer
    /// reads to say where a component came from.
    pub development: bool,
}

impl JsxSettings {
    /// The settings with this file's pragma comments applied.
    ///
    /// `@jsx`, `@jsxFrag`, `@jsxRuntime` and `@jsxImportSource` are how a file
    /// says which JSX it is written in, and a file borrowed from another
    /// project carries them. oxc takes options rather than reading comments, so
    /// they are read here.
    pub fn with_pragmas(&self, source: &str) -> Self {
        let mut settings = self.clone();
        // A file's pragmas are read in the order they are written, and each one
        // answers the whole question: naming a factory means the compiler calls
        // it, naming an import source means the compiler imports.
        for (tag, value) in pragmas(source) {
            match tag {
                "jsx" => {
                    let fragment = match &settings.function {
                        Some(JsxFunction::InScope { fragment, .. }) => fragment.clone(),
                        _ => None,
                    };
                    settings.function = Some(JsxFunction::InScope {
                        factory: value,
                        fragment,
                    });
                }
                "jsxFrag" => {
                    let factory = match &settings.function {
                        Some(JsxFunction::InScope { factory, .. }) => factory.clone(),
                        // `@jsxFrag` without `@jsx` is the default factory with
                        // a named fragment, which is what Babel does with it.
                        _ => "React.createElement".to_string(),
                    };
                    settings.function = Some(JsxFunction::InScope {
                        factory,
                        fragment: Some(value),
                    });
                }
                "jsxImportSource" => {
                    settings.function = Some(JsxFunction::Imported { source: value });
                }
                // Recognised because other toolchains write it, and it only
                // says which *kind* — the name comes from the other pragmas or
                // from the project.
                "jsxRuntime" => match (value.as_str(), &settings.function) {
                    ("classic", Some(JsxFunction::Imported { .. })) | ("classic", None) => {
                        settings.function = Some(JsxFunction::InScope {
                            factory: "React.createElement".to_string(),
                            fragment: None,
                        });
                    }
                    ("automatic", Some(JsxFunction::InScope { .. })) | ("automatic", None) => {
                        settings.function = Some(JsxFunction::Imported {
                            source: "react".to_string(),
                        });
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        settings
    }

    /// The compiler's options, or `None` when nothing has said how JSX
    /// compiles — which is a refusal rather than a default.
    fn options(&self) -> Option<oxc::transformer::JsxOptions> {
        let mut jsx = oxc::transformer::JsxOptions {
            development: self.development,
            ..oxc::transformer::JsxOptions::default()
        };
        match self.function.as_ref()? {
            JsxFunction::Imported { source } => {
                jsx.runtime = oxc::transformer::JsxRuntime::Automatic;
                jsx.import_source = Some(source.clone());
            }
            JsxFunction::InScope { factory, fragment } => {
                jsx.runtime = oxc::transformer::JsxRuntime::Classic;
                jsx.pragma = Some(factory.clone());
                jsx.pragma_frag = fragment.clone();
            }
        }
        Some(jsx)
    }
}

/// What to say when a file contains JSX and nothing has said how it compiles.
///
/// Long, because it is the whole answer: there is no default to fall back on,
/// and a message that only stated the problem would leave every reader to
/// search for the two shapes.
pub fn unconfigured_jsx() -> String {
    "this file contains JSX, and nothing has said how JSX compiles here.\n\n\
     There is no default, because a default would pick a framework. Name the \
     package whose `jsx-runtime` the compiler should import from:\n\n  \
     \"jsx\": { \"importSource\": \"preact\" }\n\n\
     …or the function it should call, which the module imports itself:\n\n  \
     \"jsx\": { \"factory\": \"h\", \"fragment\": \"Fragment\" }\n\n\
     in `esdev.json`. One file can say it instead, with the pragma it \
     probably already carries:\n\n  \
     /** @jsxImportSource preact */\n  \
     /** @jsx h */"
        .to_string()
}

/// Whether a source contains JSX, parsed for the question.
///
/// For a caller holding text rather than a tree — the build's guard pass. A
/// source that does not parse is not refused here: the bundler will report the
/// syntax error itself, with its own position.
pub fn source_contains_jsx(source: &str, id: &str) -> bool {
    let path = Path::new(id);
    let Ok(source_type) = SourceType::from_path(path) else {
        return false;
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type.with_module(true)).parse();
    contains_jsx(&parsed.program)
}

/// Whether a program contains any JSX at all.
///
/// Asked of the tree rather than of the extension: a `.jsx` holding no JSX is
/// ordinary JavaScript and has nothing to configure, and a `.ts` cannot hold
/// any. Nothing else in the file matters, so the walk stops at the first one.
fn contains_jsx(program: &oxc::ast::ast::Program<'_>) -> bool {
    use oxc::ast_visit::Visit;

    #[derive(Default)]
    struct Found(bool);

    impl<'a> Visit<'a> for Found {
        fn visit_jsx_element(&mut self, _it: &oxc::ast::ast::JSXElement<'a>) {
            self.0 = true;
        }
        fn visit_jsx_fragment(&mut self, _it: &oxc::ast::ast::JSXFragment<'a>) {
            self.0 = true;
        }
    }

    let mut found = Found::default();
    found.visit_program(program);
    found.0
}

/// The message for the first decorator in a program, if it has one.
fn first_decorator(program: &oxc::ast::ast::Program<'_>) -> Option<String> {
    use oxc::ast_visit::Visit;

    #[derive(Default)]
    struct Found(bool);

    impl<'a> Visit<'a> for Found {
        fn visit_decorator(&mut self, _it: &oxc::ast::ast::Decorator<'a>) {
            self.0 = true;
        }
    }

    let mut found = Found::default();
    found.visit_program(program);
    found.0.then(unsupported_decorators)
}

/// Lowers class auto-accessors — `accessor x = 1` — into what they are defined
/// to be: a private field, and the getter and setter pair that reads it.
///
/// V8 has not shipped the keyword (it belongs to the decorators proposal), and
/// neither has the transformer this build uses, so without this a file carrying
/// one dies in the engine with `Unexpected identifier` and no idea which line
/// meant what. The rewrite runs before semantic analysis, so scopes and symbols
/// are built from the result.
///
/// A computed or private key is refused rather than guessed at: `accessor [k]`
/// has to evaluate its key exactly once, which needs a temporary in the class's
/// scope.
struct LowerAccessors<'a> {
    allocator: &'a oxc::allocator::Allocator,
    /// Whether anything was actually rewritten.
    lowered: bool,
    /// What could not be lowered, reported instead of miscompiled.
    refused: Option<String>,
}

impl<'a> oxc::ast_visit::VisitMut<'a> for LowerAccessors<'a> {
    fn visit_class(&mut self, class: &mut oxc::ast::ast::Class<'a>) {
        use oxc::allocator::Vec as ArenaVec;
        use oxc::ast::ast::*;
        use oxc::ast::builder::AstBuilder;
        use oxc::span::SPAN;

        oxc::ast_visit::walk_mut::walk_class(self, class);
        if !class
            .body
            .body
            .iter()
            .any(|element| matches!(element, ClassElement::AccessorProperty(_)))
        {
            return;
        }
        let builder = AstBuilder::new(self.allocator);
        let mut lowered: ArenaVec<'a, ClassElement<'a>> = ArenaVec::new_in(&builder);
        let elements = std::mem::replace(&mut class.body.body, ArenaVec::new_in(&builder));
        for element in elements {
            let ClassElement::AccessorProperty(accessor) = element else {
                lowered.push(element);
                continue;
            };
            let accessor = accessor.unbox();
            if !accessor.decorators.is_empty() {
                self.refused = Some(unsupported_decorators());
                return;
            }
            let name = match &accessor.key {
                PropertyKey::StaticIdentifier(ident) if !accessor.computed => ident.name,
                _ => {
                    self.refused = Some(
                        "this file has an `accessor` whose name is computed or private, which \
                         esdev does not compile yet. A plain `accessor name = …` does."
                            .to_string(),
                    );
                    return;
                }
            };
            // The backing field's name cannot collide with a private name the
            // class already has, and it is not observable: a private field is
            // reachable only from inside the class body.
            let backing = oxc::str::Ident::from_str_in(&format!("{name}_accessor"), &builder);
            lowered.push(ClassElement::new_property_definition(
                SPAN,
                PropertyDefinitionType::PropertyDefinition,
                ArenaVec::new_in(&builder),
                PropertyKey::new_private_identifier(SPAN, backing, &builder),
                None,
                accessor.value,
                false,
                accessor.r#static,
                false,
                false,
                false,
                false,
                false,
                None,
                &builder,
            ));
            let read = Expression::new_private_field_expression(
                SPAN,
                Expression::new_this_expression(SPAN, &builder),
                PrivateIdentifier::new(SPAN, backing, &builder),
                false,
                &builder,
            );
            let getter = Function::boxed(
                SPAN,
                FunctionType::FunctionExpression,
                None,
                false,
                false,
                false,
                None,
                None,
                FormalParameters::boxed(
                    SPAN,
                    FormalParameterKind::FormalParameter,
                    ArenaVec::new_in(&builder),
                    None,
                    &builder,
                ),
                None,
                Some(FunctionBody::boxed(
                    SPAN,
                    ArenaVec::new_in(&builder),
                    oxc::allocator::Vec::from_array_in(
                        [Statement::new_return_statement(SPAN, Some(read), &builder)],
                        &builder,
                    ),
                    &builder,
                )),
                &builder,
            );
            lowered.push(ClassElement::new_method_definition(
                SPAN,
                MethodDefinitionType::MethodDefinition,
                ArenaVec::new_in(&builder),
                PropertyKey::new_static_identifier(SPAN, name, &builder),
                getter,
                MethodDefinitionKind::Get,
                false,
                accessor.r#static,
                false,
                false,
                None,
                &builder,
            ));
            let written = oxc::str::Ident::from_str_in("value", &builder);
            let assign = Expression::new_assignment_expression(
                SPAN,
                AssignmentOperator::Assign,
                AssignmentTarget::new_private_field_expression(
                    SPAN,
                    Expression::new_this_expression(SPAN, &builder),
                    PrivateIdentifier::new(SPAN, backing, &builder),
                    false,
                    &builder,
                ),
                Expression::new_identifier(SPAN, written, &builder),
                &builder,
            );
            let setter = Function::boxed(
                SPAN,
                FunctionType::FunctionExpression,
                None,
                false,
                false,
                false,
                None,
                None,
                FormalParameters::boxed(
                    SPAN,
                    FormalParameterKind::FormalParameter,
                    oxc::allocator::Vec::from_array_in(
                        [FormalParameter::new(
                            SPAN,
                            ArenaVec::new_in(&builder),
                            BindingPattern::new_binding_identifier(SPAN, written, &builder),
                            None,
                            None,
                            false,
                            None,
                            false,
                            false,
                            &builder,
                        )],
                        &builder,
                    ),
                    None,
                    &builder,
                ),
                None,
                Some(FunctionBody::boxed(
                    SPAN,
                    ArenaVec::new_in(&builder),
                    oxc::allocator::Vec::from_array_in(
                        [Statement::new_expression_statement(SPAN, assign, &builder)],
                        &builder,
                    ),
                    &builder,
                )),
                &builder,
            );
            lowered.push(ClassElement::new_method_definition(
                SPAN,
                MethodDefinitionType::MethodDefinition,
                ArenaVec::new_in(&builder),
                PropertyKey::new_static_identifier(SPAN, name, &builder),
                setter,
                MethodDefinitionKind::Set,
                false,
                accessor.r#static,
                false,
                false,
                None,
                &builder,
            ));
        }
        class.body.body = lowered;
        self.lowered = true;
    }
}

/// What to say when a file uses decorators.
pub fn unsupported_decorators() -> String {
    "this file uses decorators, which esdev does not compile yet.\n\n\
     They are a stage-3 proposal the engine has not shipped, and the compiler \
     this build uses lowers only TypeScript's older, experimental form — which \
     is not the one a modern library is written in. Until it lands, a class \
     that needs one can say the same thing in plain JavaScript: a static \
     `properties` or `observedAttributes` in place of `@property`, and \
     `customElements.define(name, Class)` in place of `@customElement`."
        .to_string()
}

/// The `@jsx…` pragmas in a source's comments, in the order they appear.
///
/// Scanned rather than parsed: a pragma is only ever read out of a comment, and
/// the shapes that matter — `/** @jsx h */`, `// @jsxImportSource preact` — are
/// a tag and the word after it. A match inside a string is possible in
/// principle and has never been seen in practice; the cost of being wrong is
/// that a file compiles the way it asked to.
fn pragmas(source: &str) -> Vec<(&'static str, String)> {
    const TAGS: [&str; 4] = ["jsxImportSource", "jsxRuntime", "jsxFrag", "jsx"];
    let mut found = Vec::new();
    // Only the head of the file: a pragma is a file-level declaration, and
    // scanning a megabyte of bundled output for one is wasted work.
    let head = &source[..source.len().min(4096)];
    let mut at = 0;
    while let Some(index) = head[at..].find('@') {
        let start = at + index + 1;
        at = start;
        let Some(tag) = TAGS.iter().find(|tag| head[start..].starts_with(**tag)) else {
            continue;
        };
        let rest = head[start + tag.len()..].trim_start_matches([' ', '\t']);
        let value: String = rest
            .chars()
            .take_while(|character| !character.is_whitespace() && *character != '*')
            .collect();
        if !value.is_empty() {
            found.push((*tag, value));
        }
    }
    found
}

impl TypeStripper {
    /// The stripper everything but a `--setup` test run uses.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, compiling JSX the way the project said to.
    pub fn with_jsx(jsx: JsxSettings) -> Self {
        Self {
            jsx,
            ..Self::default()
        }
    }

    /// Names how JSX compiles on a stripper that already exists.
    #[must_use]
    pub fn compiling_jsx(mut self, jsx: JsxSettings) -> Self {
        self.jsx = jsx;
        self
    }

    /// A stripper that imports `modules` before `entry` runs.
    ///
    /// **Prepended, and on the entry's own first line.** A setup file exists to
    /// have happened *before* anything else — a global stubbed, a polyfill
    /// installed — and an import appended at the end evaluates after the test
    /// file's own imports, which is after the module under test has already
    /// read whatever the setup was going to change. Prepending puts it first.
    ///
    /// It costs no line numbers, which is the property D71 is built on: the
    /// prelude carries no newline, so the file's line 1 is still line 1 and a
    /// failing assertion still names the line the developer wrote. Only the
    /// columns on that one line move.
    pub fn before(entry: &Path, modules: Vec<String>) -> Self {
        Self {
            // `file://{path.display()}` is not a file URL on Windows: its
            // drive letter becomes the host or scheme rather than the path.
            // The loader names modules with `Url::from_file_path`, so use that
            // exact spelling for the entry the prelude belongs to.
            prelude: Some((
                url::Url::from_file_path(entry)
                    .map(|url| url.to_string())
                    .unwrap_or_else(|()| format!("file://{}", entry.display())),
                modules,
            )),
            jsx: JsxSettings::default(),
        }
    }

    /// Adds one module before the existing prelude. The DOM has to exist
    /// before a user setup module evaluates, because setup commonly imports
    /// helpers that read `document` at module scope.
    pub fn before_with(entry: &Path, first: Option<String>, modules: Vec<String>) -> Self {
        let mut prelude = first.into_iter().collect::<Vec<_>>();
        prelude.extend(modules);
        Self::before(entry, prelude)
    }

    /// The prelude for this module, or empty for every other module.
    fn prelude_for(&self, specifier: &str, path: &str) -> String {
        let Some((entry, modules)) = &self.prelude else {
            return String::new();
        };
        let mine = entry == specifier
            || entry
                .strip_prefix("file://")
                .is_some_and(|file| file == path);
        if !mine {
            return String::new();
        }
        modules
            .iter()
            .map(|module| format!("import {};", serde_json::Value::String(module.clone())))
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Whether a module id names a file this transform has anything to do with.
///
/// Plain `.js`/`.mjs` is returned untouched rather than round-tripped through
/// the parser and printer: reprinting is not free, and every byte it changed
/// would be a byte the stack traces no longer match.
fn needs_transform(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "mts" | "cts" | "jsx")
    )
}

/// The first error-severity diagnostic, formatted, or `None` if the batch holds
/// only advisories. The first is the one that matters — the rest are usually
/// cascades from it.
fn first_error(diagnostics: &[oxc::diagnostics::OxcDiagnostic]) -> Option<String> {
    diagnostics
        .iter()
        .find(|d| d.severity == oxc::diagnostics::Severity::Error)
        .map(|d| format!("{d}"))
}

impl SourceTransform for TypeStripper {
    fn reserved_query(&self) -> Option<&'static str> {
        Some(crate::module_mocks::ACTUAL)
    }

    fn transform(&self, specifier: &str, source: String) -> Result<String, String> {
        let output = self.transform_source(specifier, source)?;
        // Under coverage, what V8 is about to compile, and where it came from.
        if crate::coverage::collect::recording() {
            let path = specifier.strip_prefix("file://").map_or(specifier, |rest| {
                rest.split(['?', '#']).next().unwrap_or(rest)
            });
            let prelude = self.prelude_for(specifier, path);
            crate::coverage::collect::record(
                specifier,
                crate::coverage::collect::Executed {
                    text: output.clone(),
                    prelude: u32::try_from(prelude.encode_utf16().count()).unwrap_or(0),
                    mappings: PRINTED.with_borrow_mut(Option::take),
                },
            );
        }
        Ok(output)
    }
}

thread_local! {
    /// The mappings of the program [`print`] last printed, for coverage.
    static PRINTED: std::cell::RefCell<Option<Vec<[u32; 4]>>> =
        const { std::cell::RefCell::new(None) };
}

impl TypeStripper {
    fn transform_source(&self, specifier: &str, source: String) -> Result<String, String> {
        PRINTED.with_borrow_mut(|printed| *printed = None);
        // A mocked module is replaced whole (D101); its real source is loaded
        // under the reserved query, which is a different id.
        if let Some(mocked) = crate::module_mocks::synthetic(specifier) {
            return Ok(mocked);
        }
        // The specifier is a file: URL; oxc wants a path, and only to read the
        // extension off it. A URL that will not convert (there should be none —
        // the loader produces file: URLs) is left alone rather than guessed at.
        let path = match specifier.strip_prefix("file://") {
            Some(rest) => rest.split(['?', '#']).next().unwrap_or(rest),
            None => specifier,
        };
        // Applied to the *output* below, never to the input. Prepending it here
        // would put it through the printer, which lays a statement out on a
        // line of its own — and the file would run one line lower than it was
        // written, which is the property D71 is built on. A plain `.js` is not
        // reprinted at all, so it takes the prelude directly.
        let prelude = self.prelude_for(specifier, path);
        // `mock.module` calls resolved from this file, and hoisted above its
        // imports when they are at its top level (D101).
        let source = crate::module_mocks::rewrite(&source, Path::new(path)).unwrap_or(source);
        // A plain `.js` is left alone unless it might hold an auto-accessor,
        // which the engine cannot parse. The word is the trigger, not the
        // answer: the parse below decides, and a file that turns out to have
        // none is returned byte for byte as it always was.
        let javascript = !needs_transform(path);
        if javascript && !source.contains("accessor") {
            return Ok(if prelude.is_empty() {
                source
            } else {
                format!("{prelude}{source}")
            });
        }

        let path = Path::new(path);
        let source_type = SourceType::from_path(path)
            .map_err(|e| format!("cannot determine the source type: {e}"))?
            // Always a module. This runtime has no script goal and no CommonJS
            // (D22), so a `.ts` here is an ES module regardless of what the
            // extension would mean to Node.
            .with_module(true);

        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &source, source_type).parse();
        // Only genuine errors stop the run: the parser also reports advisory
        // diagnostics, and refusing to run a file over one of those would be a
        // typechecker's behaviour, which this deliberately is not.
        if let Some(error) = first_error(&parsed.diagnostics) {
            return Err(error);
        }
        let mut program = parsed.program;

        // Decorators and auto-accessors, neither of which the engine has: one
        // is refused with what to write instead, the other is lowered into the
        // private field and accessor pair it is defined to be.
        if let Some(decorator) = first_decorator(&program) {
            return Err(decorator);
        }
        let lowered = {
            use oxc::ast_visit::VisitMut;
            let mut lowering = LowerAccessors {
                allocator: &allocator,
                lowered: false,
                refused: None,
            };
            lowering.visit_program(&mut program);
            if let Some(refused) = lowering.refused {
                return Err(refused);
            }
            lowering.lowered
        };
        // The word was in a comment or a name, so this is the ordinary
        // JavaScript file it was before: hand back the bytes, not a reprint.
        if javascript && !lowered {
            return Ok(if prelude.is_empty() {
                source
            } else {
                format!("{prelude}{source}")
            });
        }

        // The transformer needs scoping information to rename and resolve as it
        // erases; `SemanticBuilder` is what produces it.
        //
        // `with_enum_eval` is not optional despite reading like a tuning knob:
        // a TypeScript `enum` compiles to an IIFE whose member values may refer
        // to earlier members, so the transformer needs their evaluated constants
        // and *panics* without them. Anything short of enabling it turns a
        // one-line `enum` in a user's file into a crash.
        let scoping = SemanticBuilder::new()
            .with_enum_eval(true)
            .build(&program)
            .semantic
            .into_scoping();

        let settings = self.jsx.with_pragmas(&source);
        let Some(jsx) = settings.options() else {
            // Only a file that actually contains JSX is refused; a `.jsx` that
            // holds none is JavaScript with an unusual extension.
            if contains_jsx(&program) {
                return Err(unconfigured_jsx());
            }
            let result = Transformer::new(&allocator, path, &TransformOptions::default())
                .build_with_scoping(scoping, &mut program);
            if let Some(error) = first_error(&result.diagnostics) {
                return Err(error);
            }
            return Ok(format!("{prelude}{}", print(&program, path)));
        };
        let options = TransformOptions {
            jsx,
            ..TransformOptions::default()
        };
        let result =
            Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
        if let Some(error) = first_error(&result.diagnostics) {
            return Err(error);
        }

        // No newline between them: the prelude shares line 1 with whatever the
        // printer put there, so every line below keeps the number it had.
        Ok(format!("{prelude}{}", print(&program, path)))
    }
}

/// Prints a transformed program, and records where each of its positions came
/// from. The printer lays the code out afresh, so after the first stripped type
/// a line no longer is the line that was written; the map is how a stack frame
/// naming the file is put back on the line that was, by the same remapping an
/// uncaught error's stack goes through.
fn print(program: &oxc::ast::ast::Program<'_>, path: &Path) -> String {
    let printed = Codegen::new()
        .with_options(oxc::codegen::CodegenOptions {
            source_map_path: Some(path.to_path_buf()),
            ..oxc::codegen::CodegenOptions::default()
        })
        .build(program);
    if let Some(map) = printed.map {
        es_runtime_cli_common::sourcemap::register(path, &map.to_json_string());
        if crate::coverage::collect::recording() {
            let mappings = map
                .get_tokens()
                .map(|token| {
                    [
                        token.get_dst_line(),
                        token.get_dst_col(),
                        token.get_src_line(),
                        token.get_src_col(),
                    ]
                })
                .collect();
            PRINTED.with_borrow_mut(|printed| *printed = Some(mappings));
        }
    }
    printed.code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(name: &str, source: &str) -> Result<String, String> {
        TypeStripper::new().transform(&format!("file:///{name}"), source.to_string())
    }

    fn strip_with(jsx: JsxSettings, name: &str, source: &str) -> Result<String, String> {
        TypeStripper::with_jsx(jsx).transform(&format!("file:///{name}"), source.to_string())
    }

    #[test]
    fn a_javascript_file_is_returned_untouched() {
        // Byte-identical, not merely equivalent: reprinting a file nobody asked
        // to change would move every stack frame in it.
        let source = "const   x =  1;\n\n// a comment\nexport default x;\n";
        assert_eq!(strip("app.js", source).unwrap(), source);
        assert_eq!(strip("app.mjs", source).unwrap(), source);
    }

    #[test]
    fn type_annotations_are_erased() {
        let out = strip(
            "app.ts",
            "const n: number = 1;\nfunction f(a: string): string { return a; }\nexport { f, n };",
        )
        .unwrap();
        assert!(!out.contains(": number"), "{out}");
        assert!(!out.contains(": string"), "{out}");
        assert!(out.contains("function f(a)"), "{out}");
        assert!(out.contains("export"), "{out}");
    }

    #[test]
    fn type_only_constructs_disappear_entirely() {
        let out = strip(
            "app.ts",
            "interface Point { x: number }\ntype Id = string;\nexport const p = { x: 1 };",
        )
        .unwrap();
        assert!(!out.contains("interface"), "{out}");
        assert!(!out.contains("type Id"), "{out}");
        assert!(out.contains("x: 1"), "{out}");
    }

    #[test]
    fn a_setup_prelude_matches_the_loader_file_url() {
        let entry = std::env::temp_dir().join("esdev_setup_prelude.ts");
        let specifier = url::Url::from_file_path(&entry)
            .expect("an absolute temporary path has a file URL")
            .to_string();
        let out = TypeStripper::before(&entry, vec!["./setup.ts".into()])
            .transform(&specifier, "export const value: number = 1;".into())
            .expect("transform");
        assert!(out.starts_with("import \"./setup.ts\";"), "{out}");
    }

    #[test]
    fn a_type_only_import_is_dropped_but_a_value_import_is_kept() {
        let out = strip(
            "app.ts",
            "import type { A } from './a.ts';\nimport { b } from './b.ts';\nexport const c = b;",
        )
        .unwrap();
        assert!(!out.contains("'./a.ts'"), "{out}");
        assert!(
            out.contains("'./b.ts'") || out.contains("\"./b.ts\""),
            "{out}"
        );
    }

    /// The transform must not touch specifiers: what a module imports has to
    /// resolve identically under `esdev` and `esrun`, or the two disagree about
    /// which file a program is.
    #[test]
    fn specifiers_are_left_exactly_as_written() {
        let out = strip(
            "app.ts",
            "import { x } from './dep.ts';\nimport { y } from 'some-pkg';\nexport const z = [x, y];",
        )
        .unwrap();
        assert!(out.contains("./dep.ts"), "{out}");
        assert!(out.contains("some-pkg"), "{out}");
    }

    #[test]
    fn tsx_gets_both_treatments() {
        let out = strip_with(
            JsxSettings {
                function: Some(JsxFunction::Imported {
                    source: "preact".to_string(),
                }),
                ..JsxSettings::default()
            },
            "app.tsx",
            "const n: number = 1;\nexport const el = <p>{n}</p>;",
        )
        .unwrap();
        assert!(!out.contains(": number"), "{out}");
        assert!(!out.contains("<p>"), "{out}");
    }

    /// TypeScript's constructs that *emit* code, rather than vanishing. These
    /// are where a stripper stops being a matter of deleting annotations, and
    /// `enum` in particular panicked the transformer until the semantic pass was
    /// told to evaluate enum members.
    #[test]
    fn an_enum_becomes_a_real_object() {
        let out = strip(
            "app.ts",
            "export enum Color { Red, Green }\nexport const g = Color.Green;",
        )
        .unwrap();
        assert!(!out.contains("enum Color"), "{out}");
        assert!(out.contains("Color"), "{out}");
    }

    #[test]
    fn an_enum_member_may_refer_to_an_earlier_one() {
        let out = strip(
            "app.ts",
            "enum E { A = 1, B = A + 1, C = B * 2 }\nexport const c = E.C;",
        )
        .unwrap();
        assert!(!out.contains("enum E"), "{out}");
    }

    #[test]
    fn a_parameter_property_becomes_an_assignment() {
        let out = strip(
            "app.ts",
            "export class Box<T> { constructor(private readonly v: T) {}\n get(): T { return this.v; } }",
        )
        .unwrap();
        assert!(!out.contains("private"), "{out}");
        assert!(!out.contains("<T>"), "{out}");
        assert!(out.contains("this.v"), "{out}");
    }

    #[test]
    fn a_namespace_is_emitted_rather_than_dropped() {
        let out = strip(
            "app.ts",
            "export namespace N { export const x = 1; }\nexport const y = N.x;",
        )
        .unwrap();
        assert!(!out.contains("namespace"), "{out}");
        assert!(out.contains("N"), "{out}");
    }

    #[test]
    fn a_syntax_error_is_reported_rather_than_swallowed() {
        let err = strip("app.ts", "const x: = ;").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn top_level_await_survives() {
        let out = strip(
            "app.ts",
            "const v: number = await Promise.resolve(1);\nexport { v };",
        )
        .unwrap();
        assert!(out.contains("await"), "{out}");
    }

    #[test]
    fn jsx_compiles_the_way_the_project_said() {
        // A factory is a function the module already has, so the call is made
        // and nothing is imported.
        let called = JsxSettings {
            function: Some(JsxFunction::InScope {
                factory: "h".to_string(),
                fragment: Some("Fragment".to_string()),
            }),
            ..JsxSettings::default()
        };
        let out = strip_with(
            called,
            "app.jsx",
            "export const el = <div id=\"a\"><>x</></div>;",
        )
        .unwrap();
        assert!(out.contains("h(\"div\""), "{out}");
        assert!(out.contains("h(Fragment"), "{out}");
        assert!(!out.contains("react"), "{out}");

        // An import source is a package the compiler imports the function from.
        let imported = JsxSettings {
            function: Some(JsxFunction::Imported {
                source: "preact".to_string(),
            }),
            ..JsxSettings::default()
        };
        let out = strip_with(imported, "app.jsx", "export const el = <div/>;").unwrap();
        assert!(out.contains("preact/jsx-runtime"), "{out}");
        assert!(!out.contains("\"react"), "{out}");
    }

    #[test]
    fn jsx_with_nothing_configured_is_refused() {
        // Guessing here would compile a Preact project into React calls, so
        // nothing is guessed.
        let err = strip("app.jsx", "export const el = <div/>;").unwrap_err();
        assert!(err.contains("nothing has said how JSX compiles"), "{err}");
        assert!(err.contains("importSource"), "{err}");
        assert!(err.contains("factory"), "{err}");

        // A file with no JSX in it is not asked the question.
        assert!(strip("app.jsx", "export const el = 1;").is_ok());
    }

    #[test]
    fn a_files_own_pragma_beats_the_project() {
        // The project imports from preact; the file names a function to call.
        let project = JsxSettings {
            function: Some(JsxFunction::Imported {
                source: "preact".to_string(),
            }),
            ..JsxSettings::default()
        };
        let out = strip_with(
            project.clone(),
            "app.jsx",
            "/** @jsx h */\n/** @jsxFrag Frag */\nexport const el = <div><>x</></div>;",
        )
        .unwrap();
        assert!(out.contains("h(\"div\""), "{out}");
        assert!(out.contains("h(Frag"), "{out}");
        assert!(!out.contains("jsx-runtime"), "{out}");

        // And the other way round: a project that names a factory, a file that
        // names an import source.
        let called = JsxSettings {
            function: Some(JsxFunction::InScope {
                factory: "h".to_string(),
                fragment: None,
            }),
            ..JsxSettings::default()
        };
        let out = strip_with(
            called,
            "app.jsx",
            "// @jsxImportSource solid-js\nexport const el = <div/>;",
        )
        .unwrap();
        assert!(out.contains("solid-js/jsx-runtime"), "{out}");
        assert!(!out.contains("h(\"div\""), "{out}");

        // A pragma is also enough on its own, in a project that said nothing.
        let out = strip("app.jsx", "/** @jsx h */\nexport const el = <div/>;").unwrap();
        assert!(out.contains("h(\"div\""), "{out}");
    }

    /// The keyword belongs to the decorators proposal, which V8 has not shipped
    /// — so a file carrying one used to die in the engine on `Unexpected
    /// identifier`, naming nothing. TypeScript, Babel and SWC all lower it, and
    /// a Lit suite of 4,000 lines could not be loaded without it.
    #[test]
    fn an_auto_accessor_becomes_a_field_and_a_pair() {
        let out = strip(
            "app.ts",
            "export class Box {\n  accessor value: number = 1;\n  static accessor shared = 2;\n}",
        )
        .unwrap();
        assert!(!out.contains("accessor value"), "{out}");
        assert!(out.contains("get value()"), "{out}");
        assert!(out.contains("set value("), "{out}");
        assert!(out.contains("#value_accessor"), "{out}");
        // The static one keeps its `static`, on all three members.
        assert_eq!(out.matches("static").count(), 3, "{out}");
    }

    #[test]
    fn a_javascript_file_with_an_accessor_is_compiled_after_all() {
        // `.js` is normally returned byte for byte, and still is when the word
        // turns out to be a comment or a name rather than the keyword.
        let out = strip("app.js", "export class Box { accessor value = 1; }").unwrap();
        assert!(out.contains("get value()"), "{out}");
        let untouched = "// accessor is only a word here\nexport const accessor = 1;\n";
        assert_eq!(strip("app.js", untouched).unwrap(), untouched);
    }

    #[test]
    fn decorators_are_refused_by_name() {
        // Guessing is not an option: the transformer this build uses lowers
        // only TypeScript's older form, which is not what a modern library is
        // written in, and compiling one as the other would change what runs.
        let err = strip("app.ts", "@tag class Box {}").unwrap_err();
        assert!(err.contains("decorators"), "{err}");
        assert!(err.contains("customElements.define"), "{err}");
        let member = strip("app.ts", "class Box { @logged run() {} }").unwrap_err();
        assert!(member.contains("decorators"), "{member}");
    }

    #[test]
    fn a_pragma_is_read_from_the_head_of_the_file() {
        let found = pragmas("/** @jsxImportSource preact */\nconst a = 1;");
        assert_eq!(found, vec![("jsxImportSource", "preact".to_string())]);
        // The longest tag wins, so `@jsxImportSource` is not read as `@jsx`.
        assert_eq!(pragmas("// @jsx h").first().unwrap().0, "jsx");
        assert!(pragmas("const a = 1;").is_empty());
        // A tag with nothing after it says nothing.
        assert!(pragmas("/** @jsx */").is_empty());
    }
}
