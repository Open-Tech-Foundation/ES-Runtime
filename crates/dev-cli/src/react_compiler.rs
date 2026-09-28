//! OXC's experimental React Compiler pass.
//!
//! The compiler runs on the original TS/JSX source before any plugin or JSX
//! lowering. Its Rust API targets OXC 0.143, the AST generation Rolldown uses;
//! the project-facing OXC transform package is a Node native addon and cannot
//! run inside esdev's ESM-only plugin isolate.

use std::path::Path;
use std::sync::Arc;

use crate::contract::{
    Answer, Context, Filter, HookSpec, Hooks, ModuleResult, Order, Pass, Pattern,
};
use oxc_143::allocator::Allocator;
use oxc_143::codegen::{Codegen, CodegenOptions};
use oxc_143::parser::Parser;
use oxc_143::semantic::SemanticBuilder;
use oxc_143::span::SourceType;
use oxc_react_compiler::{CompileResult, PluginOptions, compile};

#[derive(Debug)]
pub struct ReactCompiler {
    hooks: Hooks,
    ssr: bool,
    jsx: crate::transform::JsxSettings,
}

impl ReactCompiler {
    pub fn new(ssr: bool, jsx: crate::transform::JsxSettings) -> Self {
        Self {
            ssr,
            jsx,
            hooks: Hooks {
                transform: Some(HookSpec {
                    filter: Filter {
                        id: vec![Pattern::Regex(
                            regex::Regex::new(r"\.[jt]sx?(?:\?.*)?$")
                                .expect("static React source filter"),
                        )],
                        code: Vec::new(),
                    },
                    order: Order::Pre,
                }),
                ..Hooks::default()
            },
        }
    }
}

impl Pass for ReactCompiler {
    fn name(&self) -> &str {
        "esdev:react-compiler"
    }

    fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    fn transform<'a>(
        &'a self,
        code: &'a str,
        id: &'a str,
        module_type: &'a str,
        ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async move {
            if !matches!(
                self.jsx.with_pragmas(code).function,
                Some(crate::transform::JsxFunction::Imported { source }) if source == "react"
            ) {
                return Ok(None);
            }
            let (result, diagnostics) = transform_module(code, id, module_type, self.ssr)?;
            for diagnostic in diagnostics {
                ctx.log("warn", format!("{id}: {}", diagnostic.message));
            }
            Ok(result)
        })
    }
}

fn transform_module(
    code: &str,
    id: &str,
    module_type: &str,
    ssr: bool,
) -> Result<
    (
        Option<ModuleResult>,
        Vec<oxc_143::diagnostics::OxcDiagnostic>,
    ),
    String,
> {
    if !matches!(module_type, "js" | "jsx" | "ts" | "tsx")
        || id
            .split(['/', '\\'])
            .any(|segment| segment == "node_modules")
    {
        return Ok((None, Vec::new()));
    }
    let path = Path::new(id);
    let Ok(source_type) = SourceType::from_path(path) else {
        return Ok((None, Vec::new()));
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, source_type.with_module(true)).parse();
    // Leave syntax errors for Rolldown's own parser, which reports them with
    // the same file framing as every other build error.
    if parsed.diagnostics.has_errors() {
        return Ok((None, Vec::new()));
    }
    let mut program = parsed.program;
    let compiled = {
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&program);
        if semantic.diagnostics.has_errors() {
            return Ok((None, Vec::new()));
        }
        compile(
            &program,
            &semantic.semantic,
            &allocator,
            PluginOptions {
                output_mode: Some(if ssr {
                    oxc_react_compiler::CompilerOutputMode::Ssr
                } else {
                    oxc_react_compiler::CompilerOutputMode::Client
                }),
                ..PluginOptions::default()
            },
        )
    };

    let (output, diagnostics) = match compiled {
        CompileResult::Success {
            output,
            diagnostics,
        } => (output, diagnostics),
        CompileResult::Fatal { diagnostics } => {
            let messages = diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!("React Compiler failed for {id}:\n{messages}"));
        }
    };
    let Some(output) = output else {
        return Ok((None, diagnostics.into_iter().collect()));
    };
    output.transform(&mut program);
    let printed = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(path.to_path_buf()),
            ..CodegenOptions::default()
        })
        .build(&program);
    Ok((
        Some(ModuleResult {
            code: printed.code,
            map: printed.map.map(|map| map.to_json_string()),
            // Preserve TS/JSX module typing: Rolldown still performs its
            // normal type erasure, JSX lowering, and Fast Refresh pass.
            module_type: None,
            depends_on: Vec::new(),
        }),
        diagnostics.into_iter().collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_react_components_before_jsx_lowering_and_keeps_maps() {
        let source = r#"
export function Greeting({ name }: { name: string }) {
  return <h1>Hello {name}</h1>;
}
"#;
        let (client, diagnostics) =
            transform_module(source, "/project/src/Greeting.tsx", "tsx", false)
                .expect("compile client output");
        let client = client.expect("client component is memoized");
        assert!(
            client.code.contains("react/compiler-runtime"),
            "{}",
            client.code
        );
        assert!(client.code.contains("c("), "{}", client.code);
        assert!(client.code.contains("<h1>"), "{}", client.code);
        let map = client
            .map
            .expect("React Compiler output keeps a source map");
        rolldown_sourcemap::OwnedSourceMap::from_json_string(&map)
            .expect("React Compiler source map is valid for Rolldown");
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );

        // SSR mode applies server-specific analysis but omits client cache
        // scaffolding and its React runtime import.
        let (ssr, diagnostics) = transform_module(source, "/project/src/Greeting.tsx", "tsx", true)
            .expect("compile SSR output");
        let ssr = ssr.expect("SSR compiler output");
        assert!(!ssr.code.contains("react/compiler-runtime"), "{}", ssr.code);
        assert!(!ssr.code.contains("c("), "{}", ssr.code);
        assert!(ssr.code.contains("<h1>"), "{}", ssr.code);
        assert!(ssr.map.is_some(), "SSR compiler output keeps a source map");
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn skips_dependencies_and_non_code_modules() {
        assert!(
            transform_module(
                "export const x = 1",
                "/project/node_modules/x.ts",
                "ts",
                false
            )
            .expect("skip")
            .0
            .is_none()
        );
        assert!(
            transform_module("body", "/project/style.css", "css", false)
                .expect("skip")
                .0
                .is_none()
        );
    }
}
