use std::{rc::Rc, sync::Arc};

use llrt_modules::module_builder::ModuleBuilder;
use llrt_utils::primordials::{BasePrimordials, Primordial};
use rquickjs::{AsyncContext, AsyncRuntime, Ctx, Module, Persistent, Value};

use crate::{Argument, Error, JsValue, Result, platform};

pub struct EngineOptions {
    pub memory_limit: usize,
    pub stack_size: usize,
    /// Installed before module evaluation, including application startup work.
    pub interrupt_handler: Option<Box<dyn FnMut() -> bool + 'static>>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            memory_limit: 768 * 1024 * 1024,
            stack_size: 4 * 1024 * 1024,
            interrupt_handler: None,
        }
    }
}

/// One reusable runtime, context, and evaluated copy of youtubei.js.
#[derive(Clone)]
pub struct Engine(pub(crate) Rc<Inner>);

pub(crate) struct Inner {
    // Persistent values must be destroyed before their context/runtime.
    pub exports: Persistent<Value<'static>>,
    pub context: AsyncContext,
    pub wake: Arc<crate::wake::RuntimeWake>,
}

impl Engine {
    pub async fn new() -> Result<Self> {
        Self::with_options(EngineOptions::default()).await
    }

    pub async fn with_options(options: EngineOptions) -> Result<Self> {
        let runtime = AsyncRuntime::new()?;
        runtime
            .set_interrupt_handler(options.interrupt_handler)
            .await;
        runtime.set_memory_limit(options.memory_limit).await;
        runtime.set_max_stack_size(options.stack_size).await;
        let (resolver, loader, globals) = ModuleBuilder::default()
            .with_module(llrt_utils::module::ModuleInfo {
                name: "module",
                module: platform::BuiltinModule,
            })
            .build();
        runtime.set_loader(resolver, loader).await;
        let context = AsyncContext::full(&runtime).await?;
        let exports = context
            .with(|ctx| {
                let result = (|| {
                    BasePrimordials::init(&ctx)?;
                    globals.attach(&ctx)?;
                    platform::globals(&ctx)?;
                    let module = Module::declare(
                        ctx.clone(),
                        "youtubei",
                        include_str!(concat!(env!("OUT_DIR"), "/youtubei.js")),
                    )?;
                    let (module, evaluated) = module.eval()?;
                    evaluated.finish::<()>()?;
                    let exports = module.namespace()?;
                    platform::load(&ctx, &exports)?;
                    Ok(Persistent::save(&ctx, exports.into_value()))
                })();
                result.map_err(|e| Error::caught(&ctx, e))
            })
            .await?;
        Ok(Self(Rc::new(Inner {
            exports,
            context,
            wake: Arc::default(),
        })))
    }

    /// The complete upstream module namespace, including YT, YTNodes, Misc,
    /// Helpers, Constants, Platform, Log, and every other runtime export.
    pub fn exports(&self) -> JsValue {
        self.retain(self.0.exports.clone())
    }

    /// Resolve an exported object through property names, without evaluating JS.
    pub async fn export(&self, path: &[&str]) -> Result<JsValue> {
        let mut value = self.exports();
        for name in path {
            value = value.get(name).await?;
        }
        Ok(value)
    }

    pub async fn value(&self, value: impl Into<Argument>) -> Result<JsValue> {
        let value = value.into();
        self.value_with(|ctx| value.to_js(&ctx)).await
    }

    /// Create native callbacks, ArrayBuffers, symbols, or other QuickJS values
    /// in Rust. Do not capture engine/handles in stored callbacks (an Rc cycle).
    pub async fn value_with<F>(&self, f: F) -> Result<JsValue>
    where
        F: for<'js> FnOnce(Ctx<'js>) -> rquickjs::Result<Value<'js>>,
    {
        let value = self
            .0
            .context
            .with(|ctx| {
                f(ctx.clone())
                    .map(|v| Persistent::save(&ctx, v))
                    .map_err(|e| Error::caught(&ctx, e))
            })
            .await?;
        Ok(self.retain(value))
    }

    /// Drive background jobs (e.g. event listeners) until idle. Long-lived
    /// subscriptions should run this alongside their cancellation future.
    pub async fn idle(&self) {
        self.0.wake.run(self.0.context.runtime().idle()).await;
    }

    pub async fn run_gc(&self) {
        self.0.context.runtime().run_gc().await;
    }

    /// Interrupt CPU-bound JavaScript when the application's cancellation
    /// signal is set. HTTP cancellation remains the transport's responsibility.
    pub async fn set_interrupt_handler(&self, handler: impl FnMut() -> bool + 'static) {
        self.0
            .context
            .runtime()
            .set_interrupt_handler(Some(Box::new(handler)))
            .await;
    }

    pub(crate) fn retain(&self, value: Persistent<Value<'static>>) -> JsValue {
        JsValue {
            value,
            engine: self.clone(),
        }
    }
}
