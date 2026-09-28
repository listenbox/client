use crate::{
    Argument, BrowseOptions, Engine, GetVideoInfoOptions, JsValue, Json, Result, SessionOptions,
    models::*,
};
use rquickjs::{FromJs, Object, Persistent, Value};
use std::rc::Rc;

macro_rules! handle {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug)]
        pub struct $name(pub(crate) JsValue);

        impl $name {
            pub fn as_value(&self) -> &JsValue { &self.0 }
            pub fn engine(&self) -> &Engine { self.0.engine() }
        }
        impl From<&$name> for Argument {
            fn from(value: &$name) -> Self { value.0.clone().into() }
        }
    )+ };
}

handle!(
    Innertube,
    Session,
    Actions,
    Playlist,
    Channel,
    VideoInfo,
    Player,
    YTNode,
    Text,
    UniversalCache,
    Cache,
    FetchFunction,
    ApiResponse
);

macro_rules! scalar_properties {
    ($($name:ident: $ty:ty),+ $(,)?) => {$ (
        pub async fn $name(&self) -> Result<$ty> {
            self.0.with(|ctx, v| Object::from_js(&ctx, v)?.get(stringify!($name))).await
        }
    )+ };
}

macro_rules! object_properties {
    ($($name:ident),+ $(,)?) => {$ (
        pub async fn $name(&self) -> Result<JsValue> { self.0.get(stringify!($name)).await }
    )+ };
}

impl Innertube {
    /// Create an owned, reusable engine and upstream Innertube instance.
    pub async fn create(options: SessionOptions) -> Result<Self> {
        Self::create_in(&Engine::new().await?, options).await
    }

    /// Create another session in an existing engine without reloading the bundle.
    pub async fn create_in(engine: &Engine, options: SessionOptions) -> Result<Self> {
        let options = options.to_value(engine).await?;
        Ok(Self(
            engine
                .export(&["Innertube"])
                .await?
                .call("create", &[options.into()])
                .await?,
        ))
    }

    /// Equivalent to `new Innertube(session)`.
    pub async fn new(session: &Session) -> Result<Self> {
        Ok(Self(
            session
                .engine()
                .export(&["Innertube"])
                .await?
                .construct(&[session.into()])
                .await?,
        ))
    }

    pub async fn session(&self) -> Result<Session> {
        Ok(Session(self.0.get("session").await?))
    }
    pub async fn actions(&self) -> Result<Actions> {
        Ok(Actions(self.0.get("actions").await?))
    }
    pub async fn get_playlist(&self, id: &str) -> Result<Playlist> {
        Ok(Playlist(self.0.call("getPlaylist", &[id.into()]).await?))
    }
    pub async fn get_channel(&self, id: &str) -> Result<Channel> {
        Ok(Channel(self.0.call("getChannel", &[id.into()]).await?))
    }
    pub async fn get_basic_info(
        &self,
        id: &str,
        options: GetVideoInfoOptions,
    ) -> Result<VideoInfo> {
        Ok(VideoInfo(
            self.0
                .call(
                    "getBasicInfo",
                    &[id.into(), serde_json::to_value(options)?.into()],
                )
                .await?,
        ))
    }
    pub async fn get_info(
        &self,
        target: impl Into<Argument>,
        options: GetVideoInfoOptions,
    ) -> Result<VideoInfo> {
        Ok(VideoInfo(
            self.0
                .call(
                    "getInfo",
                    &[target.into(), serde_json::to_value(options)?.into()],
                )
                .await?,
        ))
    }
    pub async fn resolve_url(&self, url: &str) -> Result<JsValue> {
        self.0.call("resolveURL", &[url.into()]).await
    }
    pub async fn search(&self, query: &str, filters: Json) -> Result<JsValue> {
        self.0.call("search", &[query.into(), filters.into()]).await
    }
    pub async fn get_streaming_data(&self, id: &str, options: Json) -> Result<Format> {
        Format::from_value(
            self.0
                .call("getStreamingData", &[id.into(), options.into()])
                .await?,
        )
        .await
    }
    /// Returns the actual ReadableStream. Use getReader/read through JsValue,
    /// or native rquickjs typed arrays through JsValue::with.
    pub async fn download(&self, id: &str, options: Json) -> Result<JsValue> {
        self.0.call("download", &[id.into(), options.into()]).await
    }
    object_properties!(music, studio, kids, account, playlist, interact);
}

impl Session {
    pub async fn create(engine: &Engine, options: SessionOptions) -> Result<Self> {
        let options = options.to_value(engine).await?;
        Ok(Self(
            engine
                .export(&["Session"])
                .await?
                .call("create", &[options.into()])
                .await?,
        ))
    }
    pub async fn player(&self) -> Result<Option<Player>> {
        let value = self.0.get("player").await?;
        Ok(if value.is_nullish().await? {
            None
        } else {
            Some(Player(value))
        })
    }
    pub async fn set_player(&self, player: &Player) -> Result<()> {
        self.0.set("player", player).await
    }
    pub async fn cache(&self) -> Result<Option<Cache>> {
        let value = self.0.get("cache").await?;
        Ok(if value.is_nullish().await? {
            None
        } else {
            Some(Cache(value))
        })
    }
    pub async fn fetch(&self) -> Result<FetchFunction> {
        Ok(FetchFunction(
            self.0.get("http").await?.get("fetch_function").await?,
        ))
    }
    pub async fn actions(&self) -> Result<Actions> {
        Ok(Actions(self.0.get("actions").await?))
    }
    scalar_properties!(client_name: String, client_version: String, lang: String, logged_in: bool, user_agent: String);
    object_properties!(context, http, oauth);
}

impl Actions {
    /// Typed `/browse` request. The response is kept live for upstream parsers.
    pub async fn browse(&self, options: BrowseOptions) -> Result<ApiResponse> {
        Ok(ApiResponse(
            self.execute("/browse", serde_json::to_value(options)?)
                .await?,
        ))
    }
    /// Supports both raw ApiResponse and `parse: true` parsed response objects.
    pub async fn execute(&self, endpoint: &str, args: impl Into<Argument>) -> Result<JsValue> {
        self.0
            .call("execute", &[endpoint.into(), args.into()])
            .await
    }
}

impl Playlist {
    /// Batch metadata, items, alerts and continuation state into one owned read.
    pub async fn data(&self) -> Result<PlaylistData> {
        self.0.read().await
    }
    pub async fn new(
        actions: &Actions,
        response: impl Into<Argument>,
        already_parsed: bool,
    ) -> Result<Self> {
        Ok(Self(
            actions
                .engine()
                .export(&["YT", "Playlist"])
                .await?
                .construct(&[actions.into(), response.into(), already_parsed.into()])
                .await?,
        ))
    }
    pub async fn get_continuation(&self) -> Result<Self> {
        Ok(Self(self.0.call("getContinuation", &[]).await?))
    }
    pub async fn items(&self) -> Result<Vec<PlaylistItem>> {
        self.0
            .get("items")
            .await?
            .with(|ctx, v| crate::models::array(&ctx, v))
            .await
    }
    pub async fn alerts(&self) -> Result<Vec<PlaylistAlert>> {
        let value = self.0.get("page").await?.get("alerts").await?;
        if value.is_nullish().await? {
            return Ok(vec![]);
        }
        value.with(|ctx, v| crate::models::array(&ctx, v)).await
    }
    scalar_properties!(has_continuation: bool, info: PlaylistInfo);
    object_properties!(page);
}

impl Channel {
    scalar_properties!(metadata: ChannelMetadata);
}

impl VideoInfo {
    pub async fn data(&self) -> Result<VideoInfoData> {
        self.0.read().await
    }
    pub async fn new(actions: &Actions, responses: impl Into<Argument>, cpn: &str) -> Result<Self> {
        Ok(Self(
            actions
                .engine()
                .export(&["YT", "VideoInfo"])
                .await?
                .construct(&[responses.into(), actions.into(), cpn.into()])
                .await?,
        ))
    }
    pub async fn adaptive_formats(&self) -> Result<Vec<Format>> {
        self.read_formats("adaptive_formats").await
    }
    /// Progressive formats may contain both video and audio in one stream.
    pub async fn formats(&self) -> Result<Vec<Format>> {
        self.read_formats("formats").await
    }
    async fn read_formats(&self, property: &str) -> Result<Vec<Format>> {
        let values = self
            .0
            .with(|ctx, v| {
                let object = Object::from_js(&ctx, v)?;
                let Some(streaming) = object.get::<_, Option<Object>>("streaming_data")? else {
                    return Ok(vec![]);
                };
                let Some(formats) = streaming.get::<_, Option<Value>>(property)? else {
                    return Ok(vec![]);
                };
                crate::models::array::<Value>(&ctx, formats)?
                    .into_iter()
                    .map(|v| {
                        let info = FormatInfo::from_js(&ctx, v.clone())?;
                        Ok((Persistent::save(&ctx, v), info))
                    })
                    .collect::<rquickjs::Result<Vec<_>>>()
            })
            .await?;
        Ok(values
            .into_iter()
            .map(|(value, info)| Format {
                value: self.engine().retain(value),
                info: Rc::new(info),
            })
            .collect())
    }
    pub async fn microformat(&self) -> Result<Option<Microformat>> {
        self.0
            .get("page")
            .await?
            .get("0")
            .await?
            .get("microformat")
            .await?
            .read()
            .await
    }
    scalar_properties!(cpn: String, basic_info: BasicInfo, playability_status: Option<PlayabilityStatus>);
    object_properties!(streaming_data, page);
}

/// A live upstream format plus its owned metadata snapshot. All formats in a
/// response are read in one context access; selection needs no further JS calls.
#[derive(Clone, Debug)]
pub struct Format {
    value: JsValue,
    info: Rc<FormatInfo>,
}

impl From<&Format> for Argument {
    fn from(format: &Format) -> Self {
        (&format.value).into()
    }
}

impl Format {
    pub fn as_value(&self) -> &JsValue {
        &self.value
    }
    pub fn engine(&self) -> &Engine {
        self.value.engine()
    }
    /// Read-only metadata captured when this handle was returned. Raw JS
    /// mutations through as_value() do not update this snapshot.
    pub fn info(&self) -> &FormatInfo {
        &self.info
    }

    async fn from_value(value: JsValue) -> Result<Self> {
        let info = Rc::new(value.read().await?);
        Ok(Self { value, info })
    }
    pub async fn new(engine: &Engine, data: Json) -> Result<Self> {
        Self::from_value(
            engine
                .export(&["Misc", "Format"])
                .await?
                .construct(&[data.into()])
                .await?,
        )
        .await
    }
    pub async fn decipher(&self, player: Option<&Player>) -> Result<String> {
        self.value
            .call(
                "decipher",
                &[player.map(Argument::from).unwrap_or(Argument::Undefined)],
            )
            .await?
            .read()
            .await
    }
}

impl Player {
    pub async fn create(
        engine: &Engine,
        cache: Option<&Cache>,
        fetch: Option<&FetchFunction>,
        po_token: Option<&str>,
        player_id: Option<&str>,
    ) -> Result<Self> {
        Ok(Self(
            engine
                .export(&["Player"])
                .await?
                .call(
                    "create",
                    &[
                        cache.map(Argument::from).unwrap_or(Argument::Undefined),
                        fetch.map(Argument::from).unwrap_or(Argument::Undefined),
                        po_token.map(Argument::from).unwrap_or(Argument::Undefined),
                        player_id.map(Argument::from).unwrap_or(Argument::Undefined),
                    ],
                )
                .await?,
        ))
    }
    /// Upstream constructor; useful when player data is already available.
    pub async fn new(
        engine: &Engine,
        id: &str,
        signature_timestamp: u32,
        data: Json,
    ) -> Result<Self> {
        Ok(Self(
            engine
                .export(&["Player"])
                .await?
                .construct(&[id.into(), signature_timestamp.into(), data.into()])
                .await?,
        ))
    }
    scalar_properties!(url: String, player_id: String, signature_timestamp: u32);
}

impl YTNode {
    pub async fn new(engine: &Engine, class: &str, data: Json) -> Result<Self> {
        Ok(Self(
            engine
                .export(&["YTNodes", class])
                .await?
                .construct(&[data.into()])
                .await?,
        ))
    }
    /// Upstream node.is(...constructors), including checks against multiple types.
    pub async fn is(&self, classes: &[&str]) -> Result<bool> {
        let mut args = Vec::with_capacity(classes.len());
        for class in classes {
            args.push(self.engine().export(&["YTNodes", class]).await?.into());
        }
        self.0.call("is", &args).await?.read().await
    }
    pub async fn text(&self, property: &str) -> Result<Text> {
        Ok(Text(self.0.get(property).await?))
    }
}

impl Text {
    pub async fn new(engine: &Engine, data: Json) -> Result<Self> {
        Ok(Self(
            engine
                .export(&["Misc", "Text"])
                .await?
                .construct(&[data.into()])
                .await?,
        ))
    }
    pub async fn to_string(&self) -> Result<String> {
        self.0.call("toString", &[]).await?.read().await
    }
}

impl UniversalCache {
    pub fn as_cache(&self) -> Cache {
        Cache(self.0.clone())
    }
    pub async fn new(engine: &Engine, persistent: bool, directory: Option<&str>) -> Result<Self> {
        Ok(Self(
            engine
                .export(&["UniversalCache"])
                .await?
                .construct(&[
                    persistent.into(),
                    directory.map(Argument::from).unwrap_or(Argument::Undefined),
                ])
                .await?,
        ))
    }
}
