//! Owned, read-only projections of upstream parser objects used for flat
//! listings and media resolution. Unused upstream fields are intentionally
//! omitted. Conversion reads native properties/getters, never JSON.stringify.
use rquickjs::{Ctx, FromJs, Function, Object, Value, function::This};

// Keep field names aligned with upstream. Option also covers fields that the
// upstream declarations mark required but omit on continuation/unavailable pages.
macro_rules! model {
    ($(#[$meta:meta])* $name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name { $(pub $field: $ty),* }
        impl<'js> FromJs<'js> for $name {
            fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
                let object = Object::from_js(ctx, value)?;
                Ok(Self { $($field: object.get(stringify!($field))?),* })
            }
        }
    };
}

/// The result of upstream Text.toString(), evaluated while taking the snapshot.
/// Formatting, endpoints and mutation are outside this read-only projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextValue(String);

impl TextValue {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for TextValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'js> FromJs<'js> for TextValue {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value)?;
        Ok(Self(
            object
                .get::<_, Function>("toString")?
                .call((This(object),))?,
        ))
    }
}

/// A JavaScript Date snapshot. Milliseconds since the Unix epoch; NaN retains
/// JavaScript's invalid-date state without silently inventing a valid date.
#[derive(Clone, Debug, PartialEq)]
pub struct Date {
    pub timestamp_millis: f64,
}

impl<'js> FromJs<'js> for Date {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value)?;
        Ok(Self {
            timestamp_millis: object
                .get::<_, Function>("getTime")?
                .call((This(object),))?,
        })
    }
}

model!(Thumbnail { url: String, width: Option<f64>, height: Option<f64> });
model!(PlaylistInfo {
    title: Option<String>, description: Option<String>,
    thumbnails: Option<Vec<Thumbnail>>, total_items: Option<String>,
});
/// All flat page data, read together while holding the engine once.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistData {
    pub info: PlaylistInfo,
    pub items: Vec<PlaylistItem>,
    pub alerts: Vec<PlaylistAlert>,
    pub has_continuation: bool,
    pub is_complete: bool,
}

impl<'js> FromJs<'js> for PlaylistData {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value)?;
        let page: Object = object.get("page")?;
        let alerts: Value = page.get("alerts")?;
        Ok(Self {
            info: object.get("info")?,
            items: array(ctx, object.get("items")?)?,
            alerts: if alerts.is_null() || alerts.is_undefined() {
                vec![]
            } else {
                array(ctx, alerts)?
            },
            has_continuation: object.get("has_continuation")?,
            is_complete: object.get("is_complete")?,
        })
    }
}
model!(ChannelMetadata {
    title: Option<String>, description: Option<String>,
    avatar: Option<Vec<Thumbnail>>, thumbnail: Option<Vec<Thumbnail>>,
});
model!(Duration {
    text: String,
    seconds: f64
});
model!(PlaylistVideo {
    id: String, title: TextValue, video_info: TextValue, duration: Duration,
    is_playable: bool, is_live: bool, is_upcoming: bool, upcoming: Option<Date>,
});

/// Variants not consumed by the application are retained explicitly so a scan
/// can reject an incomplete listing instead of silently dropping entries.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistItem {
    PlaylistVideo(PlaylistVideo),
    LockupView(LockupView),
    Other { node_type: String },
}

impl<'js> FromJs<'js> for PlaylistItem {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value.clone())?;
        Ok(match object.get::<_, String>("type")?.as_str() {
            "PlaylistVideo" => Self::PlaylistVideo(PlaylistVideo::from_js(ctx, value)?),
            "LockupView" => Self::LockupView(LockupView::from_js(ctx, value)?),
            kind => Self::Other {
                node_type: kind.into(),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockupContentType {
    Unspecified,
    Video,
    Playlist,
    Short,
    Channel,
    Album,
    Product,
    Game,
    Clip,
    Podcast,
    Source,
    ShoppingCollection,
    Movie,
    Station,
    Show,
    Other(String),
}

impl<'js> FromJs<'js> for LockupContentType {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        Ok(match String::from_js(ctx, value)?.as_str() {
            "UNSPECIFIED" => Self::Unspecified,
            "VIDEO" => Self::Video,
            "PLAYLIST" => Self::Playlist,
            "SHORT" => Self::Short,
            "CHANNEL" => Self::Channel,
            "ALBUM" => Self::Album,
            "PRODUCT" => Self::Product,
            "GAME" => Self::Game,
            "CLIP" => Self::Clip,
            "PODCAST" => Self::Podcast,
            "SOURCE" => Self::Source,
            "SHOPPING_COLLECTION" => Self::ShoppingCollection,
            "MOVIE" => Self::Movie,
            "STATION" => Self::Station,
            "SHOW" => Self::Show,
            other => Self::Other(other.into()),
        })
    }
}

model!(LockupView {
    content_id: String, content_type: LockupContentType,
    metadata: Option<LockupMetadataView>, content_image: Option<ContentImage>,
});
model!(LockupMetadataView { title: Option<TextValue>, metadata: Option<ContentMetadataView> });
model!(ContentMetadataView { metadata_rows: Vec<MetadataRow> });
model!(MetadataRow { metadata_parts: Option<Vec<MetadataPart>> });
model!(MetadataPart { text: Option<TextValue> });
model!(ThumbnailBadgeView { text: Option<String>, badge_style: Option<String> });

#[derive(Clone, Debug, PartialEq)]
pub enum ContentImage {
    ThumbnailView { overlays: Vec<ThumbnailOverlay> },
    Other { node_type: String },
}

impl<'js> FromJs<'js> for ContentImage {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value)?;
        Ok(match object.get::<_, String>("type")?.as_str() {
            "ThumbnailView" => Self::ThumbnailView {
                overlays: array(ctx, object.get("overlays")?)?,
            },
            kind => Self::Other {
                node_type: kind.into(),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ThumbnailOverlay {
    ThumbnailOverlayBadgeView { badges: Vec<ThumbnailBadgeView> },
    ThumbnailBottomOverlayView { badges: Vec<ThumbnailBadgeView> },
    Other { node_type: String },
}

impl<'js> FromJs<'js> for ThumbnailOverlay {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value)?;
        Ok(match object.get::<_, String>("type")?.as_str() {
            "ThumbnailOverlayBadgeView" => Self::ThumbnailOverlayBadgeView {
                badges: array(ctx, object.get("badges")?)?,
            },
            "ThumbnailBottomOverlayView" => Self::ThumbnailBottomOverlayView {
                badges: array(ctx, object.get("badges")?)?,
            },
            kind => Self::Other {
                node_type: kind.into(),
            },
        })
    }
}

model!(Alert {
    alert_type: String,
    text: TextValue
});
#[derive(Clone, Debug, PartialEq)]
pub enum PlaylistAlert {
    Alert(Alert),
    AlertWithButton(Alert),
    Other { node_type: String },
}

impl<'js> FromJs<'js> for PlaylistAlert {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value.clone())?;
        Ok(match object.get::<_, String>("type")?.as_str() {
            "Alert" => Self::Alert(Alert::from_js(ctx, value)?),
            "AlertWithButton" => Self::AlertWithButton(Alert::from_js(ctx, value)?),
            kind => Self::Other {
                node_type: kind.into(),
            },
        })
    }
}

model!(BasicInfo {
    id: Option<String>, title: Option<String>, short_description: Option<String>,
    duration: Option<f64>, is_live: Option<bool>, is_upcoming: Option<bool>,
    is_live_content: Option<bool>,
});
// Upstream declares status as string, rather than a closed union.
model!(PlayabilityStatus { status: String, reason: Option<String> });
model!(PlayerMicroformat { publish_date: Option<String>, upload_date: Option<String> });

/// Metadata used during audio resolution, read in one engine access.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfoData {
    pub basic_info: BasicInfo,
    pub playability_status: Option<PlayabilityStatus>,
    pub microformat: Option<Microformat>,
    pub cpn: String,
}

impl<'js> FromJs<'js> for VideoInfoData {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value)?;
        let page: Object = object.get("page")?;
        let player: Object = page.get(0)?;
        Ok(Self {
            basic_info: object.get("basic_info")?,
            playability_status: object.get("playability_status")?,
            microformat: player.get("microformat")?,
            cpn: object.get("cpn")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Microformat {
    PlayerMicroformat(PlayerMicroformat),
    Other { node_type: String },
}

impl<'js> FromJs<'js> for Microformat {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> rquickjs::Result<Self> {
        let object = Object::from_js(ctx, value.clone())?;
        Ok(match object.get::<_, String>("type")?.as_str() {
            "PlayerMicroformat" => Self::PlayerMicroformat(PlayerMicroformat::from_js(ctx, value)?),
            kind => Self::Other {
                node_type: kind.into(),
            },
        })
    }
}

model!(AudioTrack {
    audio_is_default: bool,
    display_name: String,
    id: String
});

model!(FormatInfo {
    itag: u32, url: Option<String>, cipher: Option<String>, signature_cipher: Option<String>,
    content_length: Option<f64>, mime_type: String, bitrate: f64,
    audio_track: Option<AudioTrack>, has_audio: bool, has_video: bool, height: Option<f64>, is_type_otf: bool, drm_families: Option<Vec<String>>,
});

/// ObservedArray is a Proxy: QuickJS's native Array conversion rejects it.
pub(crate) fn array<'js, T: FromJs<'js>>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
) -> rquickjs::Result<Vec<T>> {
    let class: Object = ctx.globals().get("Array")?;
    if !class
        .get::<_, Function>("isArray")?
        .call::<_, bool>((value.clone(),))?
    {
        return Err(rquickjs::Error::new_from_js(
            value.type_of().as_str(),
            "array",
        ));
    }
    let object = Object::from_js(ctx, value)?;
    (0..object.get::<_, u32>("length")?)
        .map(|i| object.get(i))
        .collect()
}
