//! The host op behind `import.meta.resolve` for specifiers that need the module
//! loader — bare (`"lodash"`) and `#private` ones (DECISIONS D41).
//!
//! Relative, absolute-path and absolute-URL specifiers never reach here: the
//! prelude resolves those against `import.meta.url` with the realm's own `URL`,
//! with no I/O and no existence check. The rest are a `node_modules` walk or an
//! `imports` map lookup — host I/O — while `import.meta.resolve` is defined to
//! return a string, so there is nowhere to await. This op is the synchronous
//! seam, backed by [`ModuleLoader::resolve_sync`].
//!
//! Two rules make it safe to expose:
//! - **It is gated on [`Capability::FileSystem`]**, the same grant that lets a
//!   module be imported at all. `resolve` now touches the disk, so under
//!   `--deny-imports` it must fail like an import rather than become a
//!   filesystem-probing oracle for code that was denied the loader.
//! - **It answers only what `import()` would answer.** The loader runs the same
//!   resolution, root jail (D25) and import policy (D39) for both, so a resolved
//!   URL is always one the guest could have imported.
//!
//! A loader that cannot resolve synchronously (modules over the network, say)
//! returns `None`, and the op returns null — the prelude turns that back into
//! the `TypeError` that names the specifier.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{Capability, ExceptionClass};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::ModuleLoader;

use crate::Result;

/// The runtime's live view of its own loader. Shared rather than passed because
/// ops are registered at construction, while the loader arrives with the entry
/// module.
pub(crate) type LoaderSlot = Rc<RefCell<Option<Arc<dyn ModuleLoader>>>>;

/// Registers `module_resolve_sync(specifier, referrer) -> string | null`.
pub(crate) fn install(engine: &mut dyn Engine, loader: LoaderSlot) -> Result<()> {
    engine.register_op(
        OpDecl::sync("module_resolve_sync", move |args| {
            let specifier = string_arg(args.first(), "specifier")?;
            let referrer = string_arg(args.get(1), "referrer")?;

            // No loader means imports are not permitted; say that rather than
            // reporting the specifier as unresolvable.
            let Some(loader) = loader.borrow().clone() else {
                return Err(OpError::new(
                    ExceptionClass::TypeError,
                    format!(
                        "cannot resolve {specifier:?}: module loading is not permitted \
                         in this runtime"
                    ),
                ));
            };

            match loader.resolve_sync(&specifier, &referrer) {
                Some(Ok(id)) => Ok(Value::String(id)),
                Some(Err(e)) => Err(OpError::new(ExceptionClass::TypeError, e.to_string())),
                // This loader has no synchronous path; the prelude explains.
                None => Ok(Value::Null),
            }
        })
        .requires(Capability::FileSystem),
    )?;
    Ok(())
}

fn string_arg(value: Option<&Value>, name: &str) -> std::result::Result<String, OpError> {
    match value {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(OpError::type_error(format!(
            "module_resolve_sync expects a string {name}"
        ))),
    }
}
