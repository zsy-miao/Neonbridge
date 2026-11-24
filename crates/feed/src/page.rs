//! The viewer page, the login page and the web app manifest.

pub(crate) const MANIFEST: &str = r##"{"name":"Neonbridge","short_name":"Neonbridge","display":"standalone","background_color":"#08090c","theme_color":"#08090c","start_url":"./"}"##;

pub(crate) const LOGIN: &str = r##"<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover"><title>Neonbridge feed</title>
<meta name="theme-color" content="#08090c"><link rel="manifest" href="manifest.json">
<style>
:root{color-scheme:dark}*{box-sizing:border-box}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:#08090c;color:#e8eaf0;font:16px/1.45 system-ui,-apple-system,Segoe UI,sans-serif}
form{width:min(360px,92vw);display:grid;gap:12px;padding:28px;border:1px solid #23262f;border-radius:14px;background:#11131a}
h1{margin:0;font-size:20px}p{margin:0;color:#9aa0ad;font-size:14px}
input{font:inherit;padding:12px;border-radius:10px;border:1px solid #2c303b;background:#0b0d12;color:inherit}
button{font:inherit;font-weight:600;padding:12px;border:0;border-radius:10px;background:#ff2e88;color:#fff;cursor:pointer}
button:disabled{opacity:.6}.err{color:#ff7a90;font-size:14px;min-height:1em}
</style></head><body><form id="f">
<h1 id="t">Neonbridge feed</h1><p id="d">Enter the password to watch.</p>
<input id="p1" type="password" autocomplete="current-password" placeholder="Password" aria-label="Password" required>
<input id="p2" type="password" autocomplete="new-password" placeholder="Confirm password" aria-label="Confirm password" hidden>
<div class="err" id="e" role="alert"></div><button id="b">Continue</button></form>
<script>
const SETUP=__SETUP__,LOCAL=__LOCAL__,$=id=>document.getElementById(id);
if(SETUP){$('t').textContent='Create a password';
 $('d').textContent=LOCAL?'It protects the feed everywhere it is shared. At least 10 characters.':'No password is set yet. Set it on the gaming PC first (open the feed there).';
 $('p1').autocomplete='new-password';$('p2').hidden=!LOCAL;$('p2').required=LOCAL;$('b').textContent='Save and continue';if(!LOCAL)$('b').disabled=true}
$('p1').focus();
$('f').onsubmit=async e=>{e.preventDefault();$('e').textContent='';
 if(SETUP&&$('p1').value!==$('p2').value){$('e').textContent="The passwords don't match";return}
 $('b').disabled=true;
 try{const r=await fetch('auth',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'password='+encodeURIComponent($('p1').value)});
  const j=await r.json().catch(()=>({}));if(r.ok){location.reload();return}$('e').textContent=j.error||'That did not work'}
 catch(_){$('e').textContent='Not reachable'}finally{$('b').disabled=false}};
</script></body></html>"##;

pub(crate) const PAGE: &str = r##"<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover"><title>Neonbridge feed</title>
<meta name="theme-color" content="#08090c"><meta name="apple-mobile-web-app-capable" content="yes"><meta name="mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent"><link rel="manifest" href="manifest.json">
<style>
:root{color-scheme:dark}*{box-sizing:border-box}
html,body{margin:0;height:100%;background:#000;color:#e8eaf0;font:14px/1.4 system-ui,-apple-system,Segoe UI,sans-serif}
.screen{position:fixed;inset:0;display:grid;place-items:center;background:#000}
video,img{max-width:100%;max-height:100%;width:100%;height:100%;object-fit:contain;background:#000}
.bar{position:fixed;left:0;right:0;bottom:0;display:flex;gap:8px;align-items:center;padding:10px max(12px,env(safe-area-inset-right)) max(10px,env(safe-area-inset-bottom)) max(12px,env(safe-area-inset-left));
 background:linear-gradient(transparent,rgba(0,0,0,.75));transition:opacity .3s}
.bar.hide{opacity:0}.st{flex:1;color:#b9bfcc;font-variant-numeric:tabular-nums}
button{font:inherit;font-weight:600;padding:8px 12px;border:1px solid #2c303b;border-radius:9px;background:rgba(17,19,26,.85);color:#e8eaf0;cursor:pointer}
</style></head><body>
<div class="screen" id="screen"><video id="vid" muted playsinline autoplay></video><img id="shot" alt="Game feed" hidden></div>
<div class="bar" id="bar"><span class="st" id="st">Connecting...</span><button id="pip">PiP</button><button id="fs">Full screen</button></div>
<script>
const $=id=>document.getElementById(id),V=$('vid');
const MSC=window.ManagedMediaSource||window.MediaSource;let MS=null,SB=null,CODEC=null,pend=[],fails=0,gen=0,ab=null;
function say(t){$('st').textContent=t}
function push(){if(!SB||SB.updating||!pend.length)return;const n=pend.reduce((a,c)=>a+c.length,0),u=new Uint8Array(n);let o=0;
 for(const c of pend){u.set(c,o);o+=c.length}pend=[];try{SB.appendBuffer(u)}catch(e){pend.unshift(u);if(V.currentTime>3)try{SB.remove(0,V.currentTime-2)}catch(_){}}}
async function source(codec){if(MS&&SB&&CODEC===codec&&MS.readyState==='open')return;
 MS=new MSC();V.disableRemotePlayback=true;V.src=URL.createObjectURL(MS);
 await new Promise(ok=>MS.addEventListener('sourceopen',ok,{once:true}));
 SB=MS.addSourceBuffer('video/mp4; codecs="'+codec+'"');SB.mode='sequence';CODEC=codec;pend=[];
 SB.addEventListener('updateend',()=>{const b=V.buffered;if(b.length){const e=b.end(b.length-1),s=b.start(b.length-1),lag=e-V.currentTime;
  if(lag>15||V.currentTime<s)V.currentTime=Math.max(s,e-1);else V.playbackRate=lag>3?1.25:lag>1.5?1.1:1;
  if(V.paused)V.play().catch(()=>{});if(!SB.updating&&V.currentTime-b.start(0)>20){SB.remove(0,V.currentTime-8);return}}push()})}
async function video(my){let got=0;
 try{ab=new AbortController();const r=await fetch('live.mp4?r='+Date.now(),{signal:ab.signal});
  if(r.status===401){location.reload();return}if(!r.ok){say((await r.json().catch(()=>({}))).error||'No video yet');throw 0}
  await source(r.headers.get('X-Codec'));say('Live');const rd=r.body.getReader();
  for(;;){const{done,value}=await rd.read();if(done||my!==gen)break;got=1;fails=0;pend.push(value);push()}}catch(e){}
 if(my===gen&&!got)fails++;if(my===gen){say('Reconnecting...');setTimeout(()=>{if(my===gen)stream()},2000)}}
function stream(){gen++;if(ab)ab.abort();
 if(MSC&&fails<5){V.hidden=false;$('shot').hidden=true;video(gen)}
 else{V.hidden=true;$('shot').hidden=false;$('shot').src='live.mjpg?r='+Date.now();say('Live (MJPEG)');$('pip').hidden=true}}
$('shot').onerror=()=>{say('Reconnecting...');setTimeout(()=>{$('shot').src='live.mjpg?r='+Date.now()},3000)};
$('fs').onclick=()=>{const el=$('screen'),d=document;
 if(d.fullscreenElement||d.webkitFullscreenElement){(d.exitFullscreen||d.webkitExitFullscreen).call(d);return}
 const rq=el.requestFullscreen||el.webkitRequestFullscreen;if(rq){rq.call(el);return}
 if(!V.hidden&&V.webkitEnterFullscreen)V.webkitEnterFullscreen()};
$('pip').onclick=()=>{if(document.pictureInPictureElement)document.exitPictureInPicture();
 else if(V.requestPictureInPicture)V.requestPictureInPicture().catch(()=>{});else if(V.webkitSetPresentationMode)V.webkitSetPresentationMode('picture-in-picture')};
let idle;function wake(){$('bar').classList.remove('hide');clearTimeout(idle);idle=setTimeout(()=>$('bar').classList.add('hide'),3000)}
addEventListener('pointermove',wake);addEventListener('touchstart',wake);wake();
document.addEventListener('visibilitychange',()=>{if(!document.hidden&&V.paused&&!V.hidden)V.play().catch(()=>{})});
stream();
</script></body></html>"##;
