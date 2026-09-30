use std::{future::Future, rc::Rc};

use rquickjs::{
    ArrayBuffer, Ctx, Function, Object, Promise, TypedArray, Value,
    function::{Async, Constructor, Opt, This},
};

use crate::{Engine, FetchFunction, Result};

/// An owned request passed to a Rust HTTP implementation. Binary bodies and
/// headers cross the runtime boundary directly, without JSON or base64.
#[derive(Debug)]
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

pub struct FetchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Engine {
    /// Supply a Rust transport as the upstream SessionOptions.fetch function.
    /// The callback runs on the engine thread; its future may await network I/O.
    pub async fn fetch_with<F, Fut>(&self, fetch: F) -> Result<FetchFunction>
    where
        F: Fn(FetchRequest) -> Fut + 'static,
        Fut: Future<Output = Result<FetchResponse>> + 'static,
    {
        let fetch = Rc::new(fetch);
        let value = self
            .value_with(|ctx| {
                Ok(Function::new(
                    ctx,
                    Async(move |ctx, input, init| invoke(ctx, input, init, fetch.clone())),
                )?
                .into_value())
            })
            .await?;
        Ok(FetchFunction(value))
    }
}

async fn invoke<'js, F, Fut>(
    ctx: Ctx<'js>,
    input: Value<'js>,
    init: Opt<Value<'js>>,
    fetch: Rc<F>,
) -> rquickjs::Result<Value<'js>>
where
    F: Fn(FetchRequest) -> Fut,
    Fut: Future<Output = Result<FetchResponse>>,
{
    let request: Object = ctx
        .globals()
        .get::<_, Constructor>("Request")?
        .construct((input, init))?;
    let method: String = request.get("method")?;
    let url: String = request.get("url")?;
    let headers: Object = request.get("headers")?;
    let iterator: Object = headers
        .get::<_, Function>("entries")?
        .call((This(headers),))?;
    let next: Function = iterator.get("next")?;
    let mut pairs = Vec::new();
    loop {
        let step: Object = next.call((This(iterator.clone()),))?;
        if step.get::<_, bool>("done")? {
            break;
        }
        let pair: Object = step.get("value")?;
        pairs.push((pair.get(0)?, pair.get(1)?));
    }
    let body = if method == "GET" || method == "HEAD" {
        None
    } else {
        let promise: Promise = request
            .get::<_, Function>("arrayBuffer")?
            .call((This(request),))?;
        let buffer: ArrayBuffer = promise.into_future().await?;
        Some(
            buffer
                .as_bytes()
                .ok_or_else(|| rquickjs::Exception::throw_type(&ctx, "Detached request buffer"))?
                .to_vec(),
        )
    };
    let response = match fetch(FetchRequest {
        url,
        method,
        headers: pairs,
        body,
    })
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let exception = rquickjs::Exception::from_message(ctx.clone(), &error.message)?;
            if let Some(info) = error.info {
                exception
                    .as_object()
                    .set("info", ctx.json_parse(info.to_string())?)?;
            }
            return Err(exception.throw());
        }
    };
    let options = Object::new(ctx.clone())?;
    options.set("status", response.status)?;
    options.set(
        "headers",
        response
            .headers
            .into_iter()
            .map(|(key, value)| vec![key, value])
            .collect::<Vec<_>>(),
    )?;
    let body = if matches!(response.status, 204 | 205 | 304) {
        Value::new_null(ctx.clone())
    } else {
        TypedArray::<u8>::new(ctx.clone(), response.body)?.into_value()
    };
    ctx.globals()
        .get::<_, Constructor>("Response")?
        .construct((body, options))
}
