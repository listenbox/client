package youtubei

import (
	"context"
	"encoding/json"
	"errors"
	"math"
)

type Client struct{ value string }

func Web() Client               { return Client{"WEB"} }
func MobileWeb() Client         { return Client{"MWEB"} }
func EmbeddedWeb() Client       { return Client{"WEB_EMBEDDED"} }
func CreatorWeb() Client        { return Client{"WEB_CREATOR"} }
func VisionOS() Client          { return Client{"VISIONOS"} }
func TV() Client                { return Client{"TV"} }
func (c Client) String() string { return c.value }

type Thumbnail struct {
	URL    string  `json:"url"`
	Width  float64 `json:"width"`
	Height float64 `json:"height"`
}
type PlaylistItem struct {
	Type            string   `json:"type"`
	ID              string   `json:"id"`
	Title           string   `json:"title"`
	SourceTitle     *string  `json:"source_title"`
	DurationSeconds float64  `json:"duration_seconds"`
	IsPlayable      bool     `json:"is_playable"`
	ContentType     string   `json:"content_type"`
	DurationBadges  []string `json:"duration_badges"`
	Upcoming        *int64   `json:"upcoming_ms"`
}

func (item *PlaylistItem) UnmarshalJSON(raw []byte) error {
	type value PlaylistItem
	if err := json.Unmarshal(raw, (*value)(item)); err != nil {
		return err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(raw, &fields); err != nil {
		return err
	}
	if string(fields["duration_seconds"]) == "null" {
		item.DurationSeconds = math.NaN()
	}
	return nil
}

type PlaylistAlert struct {
	Type      string `json:"type"`
	AlertType string `json:"alert_type"`
}
type Playlist struct {
	owner           *Engine
	Handle          int             `json:"handle"`
	Title           string          `json:"title"`
	Thumbnails      []Thumbnail     `json:"thumbnails"`
	Items           []PlaylistItem  `json:"items"`
	Alerts          []PlaylistAlert `json:"alerts"`
	HasContinuation bool            `json:"has_continuation"`
	IsComplete      bool            `json:"is_complete"`
}
type BasicInfo struct {
	ID               string      `json:"id"`
	Title            string      `json:"title"`
	Duration         *float64    `json:"duration"`
	ShortDescription string      `json:"short_description"`
	Thumbnail        []Thumbnail `json:"thumbnail"`
	IsLive           bool        `json:"is_live"`
	IsUpcoming       bool        `json:"is_upcoming"`
}
type PlayabilityStatus struct {
	Status    string `json:"status"`
	Reason    string `json:"reason"`
	Subreason string `json:"subreason"`
}
type AudioTrack struct {
	AudioIsDefault bool   `json:"audio_is_default"`
	ID             string `json:"id"`
}
type Format struct {
	owner           *Engine
	Handle          int         `json:"handle"`
	ITag            int         `json:"itag"`
	MimeType        string      `json:"mime_type"`
	Bitrate         float64     `json:"bitrate"`
	Width           *float64    `json:"width"`
	Height          *float64    `json:"height"`
	FPS             *float64    `json:"fps"`
	ContentLength   *float64    `json:"content_length"`
	HasAudio        bool        `json:"has_audio"`
	HasVideo        bool        `json:"has_video"`
	IsTypeOTF       bool        `json:"is_type_otf"`
	DRMFamilies     []string    `json:"drm_families"`
	AudioTrack      *AudioTrack `json:"audio_track"`
	URL             string      `json:"url"`
	Cipher          string      `json:"cipher"`
	SignatureCipher string      `json:"signature_cipher"`
}
type VideoInfo struct {
	BasicInfo         BasicInfo          `json:"basic_info"`
	PlayabilityStatus *PlayabilityStatus `json:"playability_status"`
	PublishDate       string             `json:"publish_date"`
	UploadDate        string             `json:"upload_date"`
	CPN               string             `json:"cpn"`
	Formats           []Format           `json:"formats"`
}

func (format *Format) UnmarshalJSON(raw []byte) error {
	type value Format
	if err := json.Unmarshal(raw, (*value)(format)); err != nil {
		return err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(raw, &fields); err != nil {
		return err
	}
	if string(fields["content_length"]) == "null" {
		nan := math.NaN()
		format.ContentLength = &nan
	}
	return nil
}

type Session struct {
	UserAgent         string `json:"user_agent"`
	VisionOSUserAgent string `json:"visionos_user_agent"`
	TVUserAgent       string `json:"tv_user_agent"`
}
type SessionOptions struct {
	Lang                    string `json:"lang,omitempty"`
	Location                string `json:"location,omitempty"`
	Timezone                string `json:"timezone,omitempty"`
	Cookie                  string `json:"cookie,omitempty"`
	GenerateSessionLocally  bool   `json:"generate_session_locally"`
	FailFast                bool   `json:"fail_fast"`
	RetrievePlayer          bool   `json:"retrieve_player"`
	RetrieveInnertubeConfig bool   `json:"retrieve_innertube_config"`
}

func (e *Engine) Initialize(ctx context.Context, options SessionOptions) (Session, error) {
	var result Session
	err := e.Call(ctx, `__bridge.initialize`, options, &result)
	return result, err
}

func (e *Engine) PlaylistHead(ctx context.Context, id string) (Playlist, error) {
	var result Playlist
	err := e.Call(ctx, `__bridge.playlistHead`, id, &result)
	result.owner = e
	return result, err
}

func (e *Engine) Continuation(ctx context.Context, page Playlist) (Playlist, error) {
	if page.owner != nil && page.owner != e {
		return Playlist{}, errors.New("playlist belongs to a different YouTube runtime")
	}
	var result Playlist
	err := e.Call(ctx, `__bridge.continuation`, page.Handle, &result)
	result.owner = e
	return result, err
}

func (e *Engine) VideoInfo(ctx context.Context, id string, client Client) (VideoInfo, error) {
	var result VideoInfo
	err := e.Call(
		ctx,
		`__bridge.videoInfo`,
		map[string]string{"id": id, "client": client.String()},
		&result,
	)
	for i := range result.Formats {
		result.Formats[i].owner = e
	}
	return result, err
}

func (e *Engine) Decipher(ctx context.Context, format Format) (string, error) {
	if format.owner != nil && format.owner != e {
		return "", errors.New("format belongs to a different YouTube runtime")
	}
	var result string
	err := e.Call(ctx, `__bridge.decipher`, format.Handle, &result)
	return result, err
}

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
const bindings = `
const __handles = new Map();let __nextHandle=0;let __innertube;
const __retain=value=>{const id=++__nextHandle;__handles.set(id,value);return id};
const __text=value=>value?.text??null;
const __playability=status=>{if(!status)return null;const screen=status.error_screen;const message=screen?.content??screen;return {status:status.status,reason:status.reason??'',subreason:message?.subreason?.toString()??message?.reason?.toString()??''}};
const __playlist=page=>({
  handle:__retain(page),title:page.info?.title??'',thumbnails:page.info?.thumbnails??[],
  has_continuation:page.has_continuation,is_complete:page.is_complete,
  alerts:Array.from(page.page?.alerts??[],a=>({type:a.type,alert_type:a.alert_type})),
  items:Array.from(page.items,n=>{
    if(n.type==='PlaylistVideo')return {type:n.type,id:n.id,title:n.title.toString(),source_title:__text(n.title),is_playable:n.is_playable,duration_seconds:n.duration?.seconds??0,upcoming_ms:n.upcoming?.getTime()};
    if(n.type==='LockupView')return {type:n.type,id:n.content_id,title:n.metadata?.title?.toString()??'',source_title:__text(n.metadata?.title),content_type:n.content_type,duration_badges:Array.from(n.content_image?.overlays??[]).flatMap(o=>Array.from(o.badges??[],b=>b.text).filter(Boolean))};
    return {type:n.type};
  })
});
globalThis.__bridge={
  async initialize(options){
    const data=new Map();class Cache{get cache_dir(){return ''}async get(k){return data.get(k)}async set(k,v){data.set(k,v)}async remove(k){data.delete(k)}}
    YouTubeJS.Platform.load({...YouTubeJS.Platform.shim,runtime:'unknown',Cache,eval:(data,env)=>Function(...Object.keys(env),data.output)(...Object.values(env))});
    __innertube=await YouTubeJS.Innertube.create({...options,cache:new YouTubeJS.UniversalCache(false),fetch});
    return {user_agent:__innertube.session.user_agent,visionos_user_agent:YouTubeJS.Constants.CLIENTS.VISIONOS.USER_AGENT,tv_user_agent:YouTubeJS.Constants.CLIENTS.TV.USER_AGENT};
  },
  async playlistHead(id){const response=await __innertube.actions.execute('/browse',{browseId:'VL'+id,params:'wgYCCAA='});return __playlist(new YouTubeJS.YT.Playlist(__innertube.actions,response,false))},
  async continuation(handle){const page=__handles.get(handle);__handles.delete(handle);return __playlist(await page.getContinuation())},
  async videoInfo({id,client}){
    const info=await __innertube.getBasicInfo(id,{client});
    const formats=[...(info.streaming_data?.formats??[]),...(info.streaming_data?.adaptive_formats??[])];
    const micro=info.page[0].microformat;
    return {basic_info:info.basic_info,playability_status:__playability(info.playability_status),publish_date:micro?.publish_date??'',upload_date:micro?.upload_date??'',cpn:info.cpn,
      formats:formats.map(f=>({handle:__retain(f),itag:f.itag,mime_type:f.mime_type,bitrate:f.bitrate,width:f.width,height:f.height,fps:f.fps,content_length:f.content_length,has_audio:f.has_audio,has_video:f.has_video,is_type_otf:f.is_type_otf,drm_families:f.drm_families,audio_track:f.audio_track,url:f.url,cipher:f.cipher,signature_cipher:f.signature_cipher}))};
  },
  async decipher(handle){const format=__handles.get(handle);if(!format)throw new Error('Released upstream format');if((format.cipher||format.signature_cipher||(format.url&&new URL(format.url).searchParams.has('n')))&&!__innertube.session.player){__innertube.session.player=await YouTubeJS.Player.create(__innertube.session.cache,fetch)}return format.decipher(__innertube.session.player)}
};
`
