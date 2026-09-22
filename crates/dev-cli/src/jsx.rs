//! The build's JSX pass: refuses what nothing has explained, and compiles what
//! one file explained differently.
//!
//! The test runner compiles every module itself, so it can do both in its own
//! transform. A bundle cannot: the JSX pass runs inside the bundler, which reads
//! the project's settings but not a file's pragma comments, and which would
//! otherwise default to somebody's framework. So a build carries this pass.
//!
//! It does the least it can. A module the project's settings already describe is
//! left for the bundler to compile in-process; only a module whose own pragmas
//! disagree is compiled here.
//!
//! **Post order, and only for a module that is still JSX.** A project whose
//! plugin compiles `.jsx` itself — a framework compiler with its own semantics —
//! hands the bundler ordinary JavaScript, and refusing that would be refusing a
//! project that has already answered the question.

use std::sync::Arc;

use crate::contract::{Answer, Context, Filter, HookSpec, Hooks, ModuleResult, Order, Pass};
use crate::transform::{JsxSettings, TypeStripper};
use es_runtime_cli_common::run::SourceTransform;

#[derive(Debug)]
pub struct JsxPass {
    /// What the project said, which a file may still override.
    settings: JsxSettings,
    hooks: Hooks,
}

impl JsxPass {
    pub fn new(settings: JsxSettings) -> Self {
        Self {
            settings,
            hooks: Hooks {
                transform: Some(HookSpec {
                    filter: Filter::default(),
                    // After every plugin: what reaches the compiler is what
                    // matters, not what the file said on disk.
                    order: Order::Post,
                }),
                ..Hooks::default()
            },
        }
    }
}

impl Pass for JsxPass {
    fn name(&self) -> &str {
        "esdev:jsx"
    }

    fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    fn transform<'a>(
        &'a self,
        code: &'a str,
        id: &'a str,
        module_type: &'a str,
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async move {
            // Only a module the compiler will still read as JSX. A plugin that
            // turned one into `js` has answered already.
            if !matches!(module_type, "jsx" | "tsx") {
                return Ok(None);
            }
            if !crate::transform::source_contains_jsx(code, id) {
                return Ok(None);
            }
            let settings = self.settings.with_pragmas(code);
            if settings.function.is_none() {
                return Err(crate::transform::unconfigured_jsx());
            }
            if settings == self.settings {
                // The bundler was given these settings and compiles in-process,
                // which is faster than doing it here and reprinting.
                return Ok(None);
            }
            // The file disagrees with the project, and the bundler reads options
            // rather than comments — so this is where the file gets its way.
            let compiled = TypeStripper::with_jsx(settings).transform(id, code.to_string())?;
            Ok(Some(ModuleResult {
                code: compiled,
                map: None,
                module_type: Some("js".to_string()),
                depends_on: Vec::new(),
            }))
        })
    }
}
