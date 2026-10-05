use std::{cell::RefCell, collections::HashMap, rc::Rc};

use rquickjs::{
    ArrayBuffer, Ctx, Function, Object, Value,
    function::{Args, Constructor, Func, Opt, Rest, This},
};
use sha1::{Digest, Sha1};

/// The upstream CF-worker bundle probes Node's optional worker_threads via
/// createRequire. This runtime supplies native modules, not a filesystem CJS
/// loader. LLRT 0.8.1 incorrectly exports require itself as createRequire.
pub(crate) struct BuiltinModule;

impl rquickjs::module::ModuleDef for BuiltinModule {
    fn declare(declarations: &rquickjs::module::Declarations) -> rquickjs::Result<()> {
        declarations.declare("createRequire")?;
        Ok(())
    }

    fn evaluate<'js>(
        _ctx: &Ctx<'js>,
        exports: &rquickjs::module::Exports<'js>,
    ) -> rquickjs::Result<()> {
        exports.export("createRequire", Func::from(create_require))?;
        Ok(())
    }
}

fn create_require<'js>(ctx: Ctx<'js>, filename: String) -> rquickjs::Result<Function<'js>> {
    if !filename.starts_with('/')
        && !filename.starts_with("file://")
        && !std::path::Path::new(&filename).is_absolute()
    {
        return Err(rquickjs::Exception::throw_type(
            &ctx,
            "createRequire requires an absolute path or file URL",
        ));
    }
    Function::new(
        ctx,
        |ctx: Ctx<'js>, specifier: String| -> rquickjs::Result<Value<'js>> {
            let name = specifier.strip_prefix("node:").unwrap_or(&specifier);
            let available = ctx
                .userdata::<llrt_modules::module::ModuleNames>()
                .is_some_and(|modules| modules.get_list().contains(name));
            if !available {
                return Err(rquickjs::Exception::throw_reference(
                    &ctx,
                    &format!("Native module unavailable: {specifier}"),
                ));
            }
            ctx.globals()
                .get::<_, Function>("require")?
                .call((specifier,))
        },
    )
}

pub(crate) fn globals(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    // Player scripts use self-referential arrays. Browsers stringify repeated
    // join receivers as empty strings; QuickJS otherwise recurses indefinitely.
    ctx.eval::<(), _>(
        r#"
        (function () {
            "use strict";
            const nativeJoin = Array.prototype.join;
            const joining = new WeakSet();
            Array.prototype.join = function join(separator) {
                if (this === null || this === undefined)
                    return nativeJoin.call(this, separator);
                const receiver = Object(this);
                if (joining.has(receiver))
                    return "";
                joining.add(receiver);
                try {
                    return nativeJoin.call(receiver, separator);
                } finally {
                    joining.delete(receiver);
                }
            };
        })();
        "#,
    )?;
    ctx.globals()
        .set("structuredClone", Func::from(clone_value))?;
    let console = Object::new(ctx.clone())?;
    for method in ["log", "info", "warn", "error", "debug", "trace"] {
        console.set(method, Func::from(|_: Rest<Value>| ()))?;
    }
    ctx.globals().set("console", console)
}

pub(crate) fn load<'js>(ctx: &Ctx<'js>, exports: &Object<'js>) -> rquickjs::Result<()> {
    let shim = Object::new(ctx.clone())?;
    shim.set("runtime", "unknown")?;
    shim.set("server", true)?;
    for name in [
        "fetch",
        "Request",
        "Response",
        "Headers",
        "FormData",
        "File",
        "ReadableStream",
        "CustomEvent",
    ] {
        shim.set(name, ctx.globals().get::<_, Value>(name)?)?;
    }
    shim.set("uuidv4", Func::from(|| uuid::Uuid::new_v4().to_string()))?;
    shim.set(
        "sha1Hash",
        Func::from(|text: String| format!("{:x}", Sha1::digest(text.as_bytes()))),
    )?;
    shim.set("eval", Func::from(evaluate))?;
    let cache = Function::new(ctx.clone(), new_cache)?.with_constructor(true);
    shim.set("Cache", cache)?;
    let platform: Object = exports.get("Platform")?;
    platform
        .get::<_, Function>("load")?
        .call::<_, ()>((This(platform), shim))?;
    Ok(())
}

fn clone_value<'js>(
    ctx: Ctx<'js>,
    value: Value<'js>,
    options: Opt<Object<'js>>,
) -> rquickjs::Result<Value<'js>> {
    llrt_utils::clone::structured_clone(&ctx, value, options)
}

fn evaluate<'js>(
    ctx: Ctx<'js>,
    data: Object<'js>,
    env: Object<'js>,
) -> rquickjs::Result<Value<'js>> {
    let mut parameters = Args::new_unsized(ctx.clone());
    let mut arguments = Args::new_unsized(ctx.clone());
    for item in env.props::<String, Value>() {
        let (key, value) = item?;
        parameters.push_arg(key)?;
        arguments.push_arg(value)?;
    }
    parameters.push_arg(data.get::<_, String>("output")?)?;
    let constructor: Constructor = ctx.globals().get("Function")?;
    let function: Function = parameters.construct(&constructor)?;
    arguments.apply(&function)
}

fn new_cache<'js>(
    ctx: Ctx<'js>,
    persistent: Opt<Option<bool>>,
    directory: Opt<Option<String>>,
) -> rquickjs::Result<Object<'js>> {
    let cache = Object::new(ctx.clone())?;
    let directory = persistent.0.flatten().filter(|v| *v).map(|_| {
        directory
            .0
            .flatten()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("youtubei.js"))
    });
    cache.set(
        "cache_dir",
        directory
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )?;
    let storage = Rc::new(RefCell::new(HashMap::<String, Vec<u8>>::new()));
    let get_storage = storage.clone();
    let set_storage = storage.clone();
    let get_dir = directory.clone();
    let set_dir = directory.clone();
    cache.set(
        "get",
        Func::from(move |ctx: Ctx<'js>, key: String| {
            let bytes = if let Some(dir) = &get_dir {
                match std::fs::read(cache_path(dir, &key)) {
                    Ok(bytes) => Some(bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(rquickjs::Exception::throw_message(&ctx, &e.to_string())),
                }
            } else {
                get_storage.borrow().get(&key).cloned()
            };
            bytes.map(|v| ArrayBuffer::new(ctx.clone(), v)).transpose()
        }),
    )?;
    cache.set(
        "set",
        Func::from(move |ctx: Ctx<'js>, key: String, bytes: ArrayBuffer<'js>| {
            let bytes = bytes
                .as_bytes()
                .ok_or_else(|| rquickjs::Exception::throw_type(&ctx, "Detached cache buffer"))?
                .to_vec();
            if let Some(dir) = &set_dir {
                std::fs::create_dir_all(dir)
                    .and_then(|_| std::fs::write(cache_path(dir, &key), bytes))
                    .map_err(|e| rquickjs::Exception::throw_message(&ctx, &e.to_string()))?;
            } else {
                set_storage.borrow_mut().insert(key, bytes);
            }
            Ok::<_, rquickjs::Error>(())
        }),
    )?;
    cache.set(
        "remove",
        Func::from(move |ctx: Ctx<'js>, key: String| {
            if let Some(dir) = &directory {
                match std::fs::remove_file(cache_path(dir, &key)) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(rquickjs::Exception::throw_message(&ctx, &e.to_string())),
                }
            } else {
                storage.borrow_mut().remove(&key);
            }
            Ok(())
        }),
    )?;
    Ok(cache)
}

fn cache_path(dir: &std::path::Path, key: &str) -> std::path::PathBuf {
    dir.join(format!("{:x}", Sha1::digest(key.as_bytes())))
}
