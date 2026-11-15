package youtubei

//nolint:lll // Preserve embedded JavaScript, vector assets or independent wire fixtures verbatim.
const hostPrimitives = `
globalThis.__goErrorID = function find(error,seen=new Set()){
  if(!error||typeof error!=='object'||seen.has(error))return 0;
  seen.add(error);if(error.__goError)return error.__goError;
  return find(error.cause,seen)||find(error.info?.error,seen)||find(error.info?.cause,seen);
};
globalThis.self = globalThis;
globalThis.window = globalThis;
globalThis.require = specifier => {
  if(specifier==='module'||specifier==='node:module')return {createRequire:path=>{
    if(typeof path!=='string'||(!path.startsWith('/')&&!path.startsWith('file://')))throw new TypeError('createRequire requires an absolute path or file URL');
    return name=>{if(name==='buffer'||name==='node:buffer')return {Buffer:globalThis.Buffer};throw new ReferenceError('Native module unavailable: '+name)};
  }};
  throw new ReferenceError('Native module unavailable: '+specifier);
};
globalThis.console = Object.fromEntries(['log','info','warn','error','debug','trace'].map(k=>[k,()=>{}]));
const toBase64 = value => {
  const bytes = value instanceof ArrayBuffer ? new Uint8Array(value) : new Uint8Array(value.buffer,value.byteOffset,value.byteLength);
  let text=''; for(let i=0;i<bytes.length;i++)text+=String.fromCharCode(bytes[i]); return btoa(text);
};
const fromBase64 = value => Uint8Array.from(atob(value||''),c=>c.charCodeAt(0));
globalThis.TextEncoder = class TextEncoder {
  get encoding(){return 'utf-8'}
  encode(value=''){return new Uint8Array(__textEncode(String(value)))}
  encodeInto(value,dest){const encoded=this.encode(value);const count=Math.min(encoded.length,dest.length);dest.set(encoded.subarray(0,count));return {read:new TextDecoder().decode(encoded.subarray(0,count)).length,written:count}}
};
globalThis.TextDecoder = class TextDecoder {
  constructor(encoding='utf-8'){if(!['utf-8','utf8'].includes(encoding.toLowerCase()))throw new TypeError('Unsupported encoding')}
  decode(value){return value ? __textDecode(toBase64(value)) : ''}
};
const encodeForm = value => encodeURIComponent(value).replace(/%20/g,'+').replace(/[!'()~]/g,c=>'%'+c.charCodeAt(0).toString(16).toUpperCase());
const decodeForm = value => {try{return decodeURIComponent(value.replace(/\+/g,' '))}catch{return value}};
globalThis.URLSearchParams = class URLSearchParams {
  constructor(init='',changed=()=>{}){
    this.changed=changed;
    if(typeof init==='string') this.pairs=init.replace(/^\?/,'').split('&').filter(Boolean).map(p=>{const i=p.indexOf('=');return i<0?[decodeForm(p),'']:[decodeForm(p.slice(0,i)),decodeForm(p.slice(i+1))]});
    else if(init[Symbol.iterator])this.pairs=Array.from(init,([k,v])=>[String(k),String(v)]);
    else this.pairs=Object.entries(init).map(([k,v])=>[k,String(v)]);
  }
  get size(){return this.pairs.length}
  append(k,v){this.pairs.push([String(k),String(v)]);this.changed(this.toString())}
  delete(k,v){this.pairs=this.pairs.filter(p=>p[0]!==String(k)||(v!==undefined&&p[1]!==String(v)));this.changed(this.toString())}
  get(k){return this.pairs.find(p=>p[0]===String(k))?.[1]??null}
  getAll(k){return this.pairs.filter(p=>p[0]===String(k)).map(p=>p[1])}
  has(k,v){return this.pairs.some(p=>p[0]===String(k)&&(v===undefined||p[1]===String(v)))}
  set(k,v){let found=false;this.pairs=this.pairs.filter(p=>{if(p[0]!==String(k))return true;if(found)return false;p[1]=String(v);found=true;return true});if(!found)this.pairs.push([String(k),String(v)]);this.changed(this.toString())}
  sort(){this.pairs.sort((a,b)=>a[0]<b[0]?-1:a[0]>b[0]?1:0);this.changed(this.toString())}
  *entries(){yield* this.pairs} *keys(){for(const p of this.pairs)yield p[0]} *values(){for(const p of this.pairs)yield p[1]}
  [Symbol.iterator](){return this.entries()}
  forEach(fn,thisArg){for(const [k,v] of this.pairs)fn.call(thisArg,v,k,this)}
  toString(){return this.pairs.map(([k,v])=>encodeForm(k)+'='+encodeForm(v)).join('&')}
};
globalThis.URL = class URL {
  constructor(value,base){this._parts=__parseURL(String(value),base===undefined?'':String(base));this._params=null}
  get href(){const p=this._parts;let auth=p.username?encodeURIComponent(p.username)+(p.password?':'+encodeURIComponent(p.password):'')+'@':'';return p.protocol+'//'+auth+p.host+p.pathname+p.search+p.hash}
  set href(v){this._parts=__parseURL(String(v));this._params=null}
  get origin(){return this._parts.protocol+'//'+this._parts.host}
  get searchParams(){if(!this._params)this._params=new URLSearchParams(this._parts.search,v=>{this._parts.search=v?'?'+v:''});return this._params}
  toString(){return this.href} toJSON(){return this.href}
  static canParse(v,b){try{new URL(v,b);return true}catch{return false}}
  static parse(v,b){try{return new URL(v,b)}catch{return null}}
};
for(const key of ['protocol','username','password','host','hostname','port','pathname','search','hash'])Object.defineProperty(URL.prototype,key,{get(){return this._parts[key]},set(value){value=String(value);const p=this._parts;if(key==='search'){p.search=value&&!value.startsWith('?')?'?'+value:value;this._params=null}else if(key==='hash'){p.hash=value&&!value.startsWith('#')?'#'+value:value}else if(key==='hostname'){p.hostname=value;p.host=value+(p.port?':'+p.port:'')}else if(key==='port'){p.port=value;p.host=p.hostname+(value?':'+value:'')}else if(key==='host'){p.host=value}else p[key]=value}});
globalThis.Headers=class Headers{
  constructor(init){this.pairs=[];if(init instanceof Headers)init=init.pairs;if(init){if(init[Symbol.iterator])for(const [k,v] of init)this.append(k,v);else for(const [k,v] of Object.entries(init))this.append(k,v)}}
  append(k,v){k=String(k).toLowerCase();v=String(v).trim();const p=this.pairs.find(p=>p[0]===k);if(p)p[1]+=', '+v;else this.pairs.push([k,v])}
  set(k,v){this.delete(k);this.append(k,v)} delete(k){this.pairs=this.pairs.filter(p=>p[0]!==String(k).toLowerCase())}
  get(k){return this.pairs.find(p=>p[0]===String(k).toLowerCase())?.[1]??null} has(k){return this.get(k)!==null}
  *entries(){yield* this.pairs} *keys(){for(const p of this.pairs)yield p[0]} *values(){for(const p of this.pairs)yield p[1]}
  [Symbol.iterator](){return this.entries()} forEach(fn,arg){for(const [k,v] of this.pairs)fn.call(arg,v,k,this)}
};
const bodyBytes = value => value==null?new Uint8Array():typeof value==='string'?new TextEncoder().encode(value):value instanceof ArrayBuffer?new Uint8Array(value):ArrayBuffer.isView(value)?new Uint8Array(value.buffer,value.byteOffset,value.byteLength):new TextEncoder().encode(String(value));
globalThis.Buffer=class Buffer extends Uint8Array {
  static from(value,encoding='utf8'){
    if(typeof value!=='string')return new Buffer(value instanceof ArrayBuffer?new Uint8Array(value):value);
    if(encoding==='base64'||encoding==='base64url')return new Buffer(fromBase64(value.replace(/-/g,'+').replace(/_/g,'/')));
    if(encoding==='hex'){if(value.length%2)throw new TypeError('Invalid hex');return new Buffer(value.match(/../g)?.map(v=>parseInt(v,16))??[])}
    if(encoding!=='utf8'&&encoding!=='utf-8')throw new TypeError('Unsupported buffer encoding');return new Buffer(new TextEncoder().encode(value));
  }
  static alloc(length,fill=0){return new Buffer(length).fill(fill)}
  static isBuffer(value){return value instanceof Buffer}
  static byteLength(value,encoding){return Buffer.from(value,encoding).byteLength}
  toString(encoding='utf8'){
    if(encoding==='base64')return toBase64(this);
    if(encoding==='base64url')return toBase64(this).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
    if(encoding==='hex')return Array.from(this,b=>b.toString(16).padStart(2,'0')).join('');
    if(encoding!=='utf8'&&encoding!=='utf-8')throw new TypeError('Unsupported buffer encoding');return new TextDecoder().decode(this);
  }
};
globalThis.ReadableStream=class ReadableStream{
  constructor(source={}){this.queue=[];this.closed=false;this.source=source;this.controller={enqueue:v=>this.queue.push(v),close:()=>{this.closed=true},error:e=>{this.error=e}};this.started=Promise.resolve(source.start?.(this.controller))}
  getReader(){return {read:async()=>{await this.started;if(this.error)throw this.error;if(!this.queue.length&&!this.closed)await this.source.pull?.(this.controller);if(this.error)throw this.error;return this.queue.length?{done:false,value:this.queue.shift()}:{done:this.closed,value:undefined}},cancel:reason=>this.source.cancel?.(reason),releaseLock:()=>{}}}
  async *[Symbol.asyncIterator](){const r=this.getReader();for(;;){const n=await r.read();if(n.done)return;yield n.value}}
};
class Body{
  constructor(body){this._bytes=bodyBytes(body);this.bodyUsed=false}
  async arrayBuffer(){if(this.bodyUsed)throw new TypeError('Body already consumed');this.bodyUsed=true;return this._bytes.slice().buffer}
  async bytes(){return new Uint8Array(await this.arrayBuffer())}
  async text(){return new TextDecoder().decode(await this.arrayBuffer())}
  async json(){return JSON.parse(await this.text())}
  get body(){const bytes=this._bytes;return new ReadableStream({start(c){if(bytes.length)c.enqueue(bytes);c.close()}})}
}
globalThis.Request=class Request extends Body{
  constructor(input,options={}){super(options.body??(input instanceof Request?input._bytes:null));this.url=String(input instanceof Request?input.url:input);this.method=options.method??(input instanceof Request?input.method:'GET');this.headers=new Headers(options.headers??(input instanceof Request?input.headers:undefined));this.signal=options.signal;this.redirect=options.redirect??'follow';this.credentials=options.credentials??'same-origin'}
  clone(){return new Request(this)}
};
globalThis.Response=class Response extends Body{
  constructor(body,init={}){super(body);this.status=init.status??200;this.statusText=init.statusText??'';this.headers=new Headers(init.headers);this.url=init.url??'';this.redirected=false;this.ok=this.status>=200&&this.status<300}
  clone(){return new Response(this._bytes,{status:this.status,statusText:this.statusText,headers:this.headers,url:this.url})}
  static json(value,init={}){return new Response(JSON.stringify(value),{...init,headers:{'content-type':'application/json',...init.headers}})}
};
globalThis.fetch=async(input,init={})=>{const request=new Request(input,init);const response=await __hostFetch({url:request.url,method:request.method,headers:Array.from(request.headers),body:request._bytes.length?toBase64(request._bytes):null});return new Response(fromBase64(response.body),response)};
globalThis.crypto={randomUUID:()=>__randomUUID(),getRandomValues:array=>{array.set(fromBase64(__randomBytes(array.byteLength)));return array},subtle:{digest:async(algorithm,bytes)=>__digest(typeof algorithm==='string'?algorithm:algorithm.name,toBase64(bytes))}};
globalThis.CustomEvent=class CustomEvent{constructor(type,options={}){this.type=type;this.detail=options.detail}};
globalThis.Event=class Event{constructor(type){this.type=type}};
globalThis.EventTarget=class EventTarget{constructor(){this.listeners=new Map()}addEventListener(type,fn){if(!this.listeners.has(type))this.listeners.set(type,new Set());this.listeners.get(type).add(fn)}removeEventListener(type,fn){this.listeners.get(type)?.delete(fn)}dispatchEvent(event){for(const fn of this.listeners.get(event.type)||[])fn(event);return true}};
globalThis.Blob=class Blob extends Body{constructor(parts=[],options={}){const chunks=parts.map(bodyBytes);const bytes=new Uint8Array(chunks.reduce((n,c)=>n+c.length,0));let i=0;for(const c of chunks){bytes.set(c,i);i+=c.length}super(bytes);this.type=options.type??'';this.size=bytes.length}};
globalThis.File=class File extends Blob{constructor(parts,name,options){super(parts,options);this.name=name}};
globalThis.FormData=class FormData{constructor(){this.values=[]}append(k,v){this.values.push([k,v])}[Symbol.iterator](){return this.values[Symbol.iterator]()}};
globalThis.AbortSignal=class AbortSignal extends EventTarget{constructor(){super();this.aborted=false}throwIfAborted(){if(this.aborted)throw this.reason}};
globalThis.AbortController=class AbortController{constructor(){this.signal=new AbortSignal()}abort(reason=new Error('Aborted')){this.signal.aborted=true;this.signal.reason=reason;this.signal.dispatchEvent(new Event('abort'))}};
globalThis.structuredClone=function clone(value,seen=new Map()){if(value===null||typeof value!=='object')return value;if(seen.has(value))return seen.get(value);if(value instanceof Date)return new Date(value);if(value instanceof ArrayBuffer)return value.slice(0);if(ArrayBuffer.isView(value))return new value.constructor(value);const result=Array.isArray(value)?[]:value instanceof Map?new Map():value instanceof Set?new Set():{};seen.set(value,result);if(value instanceof Map){for(const [k,v] of value)result.set(clone(k,seen),clone(v,seen))}else if(value instanceof Set){for(const v of value)result.add(clone(v,seen))}else for(const [k,v] of Object.entries(value))result[k]=clone(v,seen);return result};
const timerCallbacks=new Map();let nextTimer=0;
globalThis.setTimeout=(fn,delay=0,...args)=>{const id=++nextTimer;timerCallbacks.set(id,()=>fn(...args));__setTimer(id,delay);return id};
globalThis.clearTimeout=id=>{timerCallbacks.delete(id);__clearTimer(id)};
globalThis.__fireTimer=id=>{const fn=timerCallbacks.get(id);timerCallbacks.delete(id);fn?.()};
globalThis.queueMicrotask=fn=>Promise.resolve().then(fn);
globalThis.caches={async open(){const data=new Map();return {match:async k=>data.get(k)?.clone(),put:async(k,v)=>{data.set(k,v.clone())},delete:async k=>data.delete(k)}}};
const joining=new WeakSet(),nativeJoin=Array.prototype.join;
Array.prototype.join=function(separator){if(this==null)return nativeJoin.call(this,separator);const receiver=Object(this);if(joining.has(receiver))return '';joining.add(receiver);try{return nativeJoin.call(receiver,separator)}finally{joining.delete(receiver)}};
`
