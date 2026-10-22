//! "Clean player" mode for the episode viewer window.
//!
//! The pirate streaming sites wrap their player in menus, comments, ads and
//! recommendations. When the setting is on, once the page of an app-owned
//! viewer window is up we inject ONE synchronous script (host-side, through
//! `scraper_engine::eval`, i.e. WebView2 `ExecuteScript`: Tauri does not inject
//! its IPC into remote pages and ExecuteScript does not await promises) that:
//!
//! * finds the player (per-site selector table below, generic fallback = the
//!   largest visible `<iframe>`/`<video>`),
//! * keeps the player, the server/mirror switcher and the ancestors of both,
//! * hides everything else with a `data-aot-hide` attribute + a style element
//!   (no node is ever removed, the site's own JS keeps working),
//! * pins the player to the window below a slim dark title bar,
//! * neutralises transparent click-hijack layers sitting on the player,
//! * re-applies itself from a throttled `MutationObserver` when the site
//!   re-renders or injects a pop-up layer.
//!
//! The script is idempotent and reversible: if no player is found, or the
//! result would be a blank window, it leaves the page exactly as it was.

use crate::adapter::all_sites;
use std::time::{Duration, Instant};

/// Selectors for one site. A selector ending in `<` means "the parent of the
/// first match" (used when only a child, e.g. an iframe, is identifiable).
pub struct SiteSelectors {
    pub site: &'static str,
    /// Candidate player roots, tried in order; first one with a real size wins.
    pub players: &'static [&'static str],
    /// Server / mirror switcher roots that stay visible (docked at the bottom).
    pub switchers: &'static [&'static str],
    /// The player is rendered client-side, so the stored episode fixture does
    /// not contain it (used by the fixture tests only); `anchor` is a selector that IS in the fixture instead.
    #[cfg_attr(not(test), allow(dead_code))]
    pub dynamic: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    pub anchor: &'static str,
}

pub const SELECTORS: &[SiteSelectors] = &[
    SiteSelectors {
        site: "animeytx",
        players: &[".aniyt-player-stage", "iframe[src*='mytsumi'] <"],
        switchers: &[".aniyt-player-shell > :not(.aniyt-player-stage)"],
        dynamic: false,
        anchor: "",
    },
    SiteSelectors {
        site: "tioanime",
        players: &["#video-container"],
        switchers: &["#episode-options"],
        dynamic: false,
        anchor: "",
    },
    SiteSelectors {
        site: "animeflv",
        players: &["#load <", "#iframePoster <"],
        switchers: &[".server-frames"],
        dynamic: false,
        anchor: "",
    },
    SiteSelectors {
        site: "jkanime",
        players: &["iframe.player_conte <"],
        switchers: &[".bg-servers"],
        dynamic: false,
        anchor: "",
    },
    SiteSelectors {
        site: "gogoanime",
        players: &["#player"],
        switchers: &["#w-servers"],
        dynamic: false,
        anchor: "",
    },
    // No stored episode fixture: selectors are a best guess, the generic
    // fallback (largest iframe/video) is what normally does the work.
    SiteSelectors {
        site: "animeland",
        players: &["#player", ".player", "#video-player"],
        switchers: &[".servers", "#servers"],
        dynamic: true,
        anchor: "",
    },
    // Player and servers are rendered by JS after load (the fixture has only
    // the static shell).
    SiteSelectors {
        site: "aniwaves",
        players: &["#player", "#w-player", ".player"],
        switchers: &["#w-servers", ".servers"],
        dynamic: true,
        anchor: "#wrapper",
    },
    SiteSelectors {
        site: "monoschinos",
        players: &["div.player[data-key]"],
        switchers: &["button.srv <"],
        dynamic: false,
        anchor: "",
    },
    SiteSelectors {
        site: "animefenix",
        players: &["#pembed"],
        switchers: &[".video-nav"],
        dynamic: false,
        anchor: "",
    },
    // React SPA: the stored fixture is the SEO shell only.
    SiteSelectors {
        site: "animoratv",
        players: &["#player", "[class*='player']"],
        switchers: &[],
        dynamic: true,
        anchor: "#root",
    },
];

fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    host.split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Last two labels (`w7.animeland.tv` -> `animeland.tv`); good enough to match
/// a mirror of the same family without a public-suffix list.
fn base_domain(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        labels.join(".")
    } else {
        labels[labels.len() - 2..].join(".")
    }
}

/// Site ids whose default domain matches `url`'s host. Empty when the URL is
/// a mirror we cannot place (callers then try every site's selectors).
pub fn site_ids_for_url(url: &str) -> Vec<&'static str> {
    let bd = base_domain(&host_of(url));
    all_sites()
        .iter()
        .filter(|s| base_domain(&host_of(s.default_base_url)) == bd)
        .map(|s| s.id)
        .collect()
}

/// JSON config handed to the page script for `url`.
pub fn config_json(url: &str) -> String {
    let ids = site_ids_for_url(url);
    let rows: Vec<&SiteSelectors> = SELECTORS
        .iter()
        .filter(|r| ids.is_empty() || ids.contains(&r.site))
        .collect();
    let players: Vec<&str> = rows.iter().flat_map(|r| r.players.iter().copied()).collect();
    let switchers: Vec<&str> = rows.iter().flat_map(|r| r.switchers.iter().copied()).collect();
    serde_json::json!({ "players": players, "switchers": switchers }).to_string()
}

/// The full script for `url`. Evaluates to a status string:
/// `ok` | `already` | `noplayer` | `challenge` | `error`.
pub fn script_for(url: &str) -> String {
    format!("({})({})", CLEAN_SCRIPT, config_json(url))
}

/// Keep applying the clean layout to `win` until it closes. Fire-and-forget
/// (runs on the async runtime, never blocks the main thread): polls the page
/// (condition-based, bounded) until a player appears, then re-checks slowly so
/// a navigation inside the window (new episode, server change) is cleaned too.
pub fn spawn_injection(win: tauri::WebviewWindow, url: String) {
    tauri::async_runtime::spawn(async move {
        let script = script_for(&url);
        let started = Instant::now();
        let mut state = PollState::default();
        loop {
            tokio::time::sleep(Duration::from_millis(if state.applied { 5000 } else { 900 })).await;
            let outcome = match crate::scraper_engine::eval(&win, &script, 5).await {
                Ok(res) => EvalOutcome::Status(res),
                Err(_) => EvalOutcome::Error,
            };
            if state.next(outcome, started.elapsed()) == PollAction::Stop {
                return;
            }
        }
    });
}

#[derive(Debug, Clone, PartialEq)]
enum EvalOutcome {
    Status(String),
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PollAction {
    Continue,
    Stop,
}

/// Poll bookkeeping, pure so the stop rules are testable.
#[derive(Debug, Default)]
struct PollState {
    applied: bool,
    ever_applied: bool,
    consecutive_errors: u32,
}

/// A single eval timeout (busy page) must not end the loop; a closed window
/// fails every eval, so 3 in a row means it is gone.
const MAX_CONSECUTIVE_ERRORS: u32 = 3;
const GIVE_UP_WITHOUT_PLAYER: Duration = Duration::from_secs(120);

impl PollState {
    fn next(&mut self, outcome: EvalOutcome, elapsed: Duration) -> PollAction {
        match outcome {
            EvalOutcome::Error => {
                self.consecutive_errors += 1;
                if self.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    return PollAction::Stop;
                }
            }
            EvalOutcome::Status(res) => {
                self.consecutive_errors = 0;
                if res.contains("\"ok\"") || res.contains("\"already\"") {
                    self.applied = true;
                    self.ever_applied = true;
                } else {
                    // The page lost (or never had) a player: back to fast polling.
                    self.applied = false;
                    if elapsed > GIVE_UP_WITHOUT_PLAYER && !self.ever_applied {
                        // Never found a player: give up, the page stays untouched.
                        return PollAction::Stop;
                    }
                }
            }
        }
        PollAction::Continue
    }
}

/// Page script. A function expression taking the config object. Synchronous
/// and ES5-ish on purpose (ExecuteScript does not await promises).
const CLEAN_SCRIPT: &str = r####"function(cfg){
var undoRef=null;
try{
  if(/just a moment|un momento|attention required|checking your browser/i.test(document.title||'')) return 'challenge';
  var W=window, BAR=34;
  undoRef=function(){ if(typeof undo==='function') undo(); };
  if(W.__aotClean){
    if(W.__aotClean.apply()) return 'already';
    undo(); return 'noplayer';
  }
  if(!document.body) return 'noplayer';

  function q(sel){
    sel=String(sel).replace(/^\s+|\s+$/g,'');
    var parent=false;
    if(sel.charAt(sel.length-1)==='<'){parent=true;sel=sel.slice(0,-1).replace(/\s+$/,'');}
    var e=null; try{e=document.querySelector(sel);}catch(x){}
    return parent&&e?e.parentElement:e;
  }
  function qa(sel){ try{return Array.prototype.slice.call(document.querySelectorAll(String(sel).replace(/<\s*$/,'')));}catch(x){return [];} }
  function big(e){ if(!e||e===document.body||e===document.documentElement) return false;
    var r=e.getBoundingClientRect(); return r.width>=150&&r.height>=80; }
  function area(e){ var r=e.getBoundingClientRect(); return r.width*r.height; }

  var S={player:null,media:null,title:'',sw:[]};

  function findPlayer(){
    var i,e;
    for(i=0;i<cfg.players.length;i++){ e=q(cfg.players[i]); if(big(e)) return e; }
    var best=null,ba=0,m=document.querySelectorAll('iframe,video');
    for(i=0;i<m.length;i++){
      var s=m[i].getAttribute('src')||'';
      if(/^(javascript:|about:blank|blob:)/i.test(s)) continue;
      var r=m[i].getBoundingClientRect(), a=r.width*r.height;
      if(a>ba&&r.width>=200&&r.height>=110){ba=a;best=m[i];}
    }
    return best;
  }
  function pickTitle(){
    var h=document.querySelector('h1'), t=h&&(h.textContent||'').replace(/\s+/g,' ').replace(/^\s+|\s+$/g,'');
    if(t&&t.length<160) return t;
    t=(document.title||'').replace(/\s+/g,' ');
    var parts=t.split(/\s+[-|–—·]\s+/);
    if(parts.length>1) parts.pop();
    return parts.join(' - ')||t;
  }
  function markChain(el,cls){
    for(var n=el;n&&n!==document.documentElement;n=n.parentElement) n.classList.add(cls);
  }
  function undo(){
    var i,l;
    var st=document.getElementById('aot-style'); if(st) st.parentNode.removeChild(st);
    var bar=document.getElementById('aot-bar'); if(bar) bar.parentNode.removeChild(bar);
    l=document.querySelectorAll('.aot-keep,.aot-player,.aot-sw,.aot-main,[data-aot-hide],[data-aot-sw]');
    for(i=0;i<l.length;i++){
      l[i].classList.remove('aot-keep','aot-player','aot-sw','aot-main'); l[i].removeAttribute('data-aot-hide');
      if(l[i].getAttribute('data-aot-sw')){ l[i].style.cssText=l[i].getAttribute('data-aot-css')||''; l[i].removeAttribute('data-aot-sw'); l[i].removeAttribute('data-aot-css'); }
    }
    document.documentElement.style.removeProperty('--aot-sw');
  }
  function css(){
    return 'html,body{margin:0!important;padding:0!important;overflow:hidden!important;background:#000!important;height:100%!important}'
    +'[data-aot-hide]{display:none!important}'
    +'.aot-keep{transform:none!important;filter:none!important;backdrop-filter:none!important;perspective:none!important;contain:none!important;will-change:auto!important}'
    +'.aot-player{position:fixed!important;top:'+BAR+'px!important;left:0!important;right:0!important;bottom:var(--aot-sw,0px)!important;width:auto!important;height:auto!important;max-width:none!important;max-height:none!important;min-height:0!important;margin:0!important;padding:0!important;border:0!important;float:none!important;aspect-ratio:auto!important;display:block!important;overflow:hidden!important;background:#000!important;z-index:2147483000!important;transform:none!important}'
    +'.aot-sw,.aot-sw *{pointer-events:auto!important}'
    +'.aot-main:not(.aot-player){position:absolute!important;top:0!important;left:0!important;width:100%!important;height:100%!important;max-width:none!important;max-height:none!important;margin:0!important;border:0!important;display:block!important}'
    +'#aot-bar{position:fixed;top:0;left:0;right:0;height:'+BAR+'px;box-sizing:border-box;padding:0 14px;background:#0d0d10;color:#e8e8ee;font:600 13px/'+BAR+'px system-ui,Segoe UI,sans-serif;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;z-index:2147483647;border-bottom:1px solid #26262c;pointer-events:none}';
  }
  function dockSwitchers(player){
    var i,j,els=[],all=[],off=0,cap=Math.floor(window.innerHeight*0.22);
    for(i=0;i<cfg.switchers.length;i++){ all=all.concat(qa(cfg.switchers[i])); }
    for(i=0;i<all.length;i++){
      var e=all[i], ok=e!==player&&!player.contains(e)&&!e.contains(player);
      for(j=0;ok&&j<all.length;j++){ if(all[j]!==e&&all[j].contains(e)) ok=false; }
      if(ok&&els.indexOf(e)<0) els.push(e);
    }
    for(i=0;i<els.length;i++){
      var el=els[i];
      markChain(el.parentElement,'aot-keep');
      if(!el.getAttribute('data-aot-sw')){
        el.setAttribute('data-aot-sw','1'); el.setAttribute('data-aot-css',el.style.cssText||'');
        el.classList.add('aot-sw');
        el.style.setProperty('position','fixed','important');
        el.style.setProperty('left','0','important');
        el.style.setProperty('right','0','important');
        el.style.setProperty('margin','0','important');
        el.style.setProperty('overflow','auto','important');
        el.style.setProperty('max-height',cap+'px','important');
        el.style.setProperty('background','#111','important');
        el.style.setProperty('z-index','2147483600','important');
        el.style.setProperty('box-sizing','border-box','important');
        el.style.setProperty('display','block','important');
      }
      el.style.setProperty('bottom',off+'px','important');
      off+=Math.min(el.getBoundingClientRect().height,cap);
    }
    document.documentElement.style.setProperty('--aot-sw',Math.min(off,cap*2)+'px');
    S.sw=els;
  }
  function hideSiblings(){
    var keep=document.querySelectorAll('.aot-keep'),i,j,c,skip=/^(SCRIPT|STYLE|LINK|META|NOSCRIPT|TEMPLATE|HEAD|TITLE)$/;
    for(i=0;i<keep.length;i++){
      var k=keep[i].children;
      for(j=0;j<k.length;j++){ c=k[j];
        if(c.classList.contains('aot-keep')||c.classList.contains('aot-player')||c.classList.contains('aot-sw')||c.id==='aot-bar'||skip.test(c.tagName)) continue;
        c.setAttribute('data-aot-hide','1');
      }
    }
  }
  function killOverlays(player){
    var pr=player.getBoundingClientRect(), pa=pr.width*pr.height, l=player.querySelectorAll('div,a,section,span'),i,n=Math.min(l.length,400);
    for(i=0;i<n;i++){
      var e=l[i]; if(e.hasAttribute('data-aot-hide')||e.closest('.aot-sw')) continue;
      var cs=getComputedStyle(e), pos=cs.position; if(pos!=='absolute'&&pos!=='fixed') continue;
      var z=parseInt(cs.zIndex,10)||0; if(pos==='absolute'&&z<100) continue;
      var r=e.getBoundingClientRect(); if(r.width*r.height<0.8*pa) continue;
      var id=(e.id||'')+' '+(typeof e.className==='string'?e.className:'');
      if(/poster|play|load|ctrl|control|sug|ifplay|server|cover|spinner|pause|error|loader|cf-|turnstile/i.test(id)) continue;
      if(e.querySelector('iframe,video,button,input,svg,img,canvas')) continue;
      if((e.textContent||'').replace(/\s+/g,'').length>0) continue;
      var bg=cs.backgroundColor; if(!(parseFloat(cs.opacity)<0.05||bg==='transparent'||/rgba\(.*,\s*0\)$/.test(bg))) continue;
      e.setAttribute('data-aot-hide','1');
    }
    var f=player.querySelectorAll('iframe');
    for(i=0;i<f.length;i++){ if(f[i]!==S.media&&/^(blob:|javascript:|about:blank)/i.test(f[i].getAttribute('src')||'')) f[i].setAttribute('data-aot-hide','1'); }
  }
  // A switcher must receive clicks: if some other layer sits on top of it
  // (ad shim, transparent hijack), hide that layer. Nodes are never moved.
  function unblockSwitchers(){
    var i;
    for(i=0;i<S.sw.length;i++){
      var n=0, el=S.sw[i], r=el.getBoundingClientRect();
      if(r.width<4||r.height<4) continue;
      var x=r.left+Math.min(r.width/2,40), y=r.top+r.height/2, top=document.elementFromPoint(x,y);
      while(top&&!el.contains(top)&&top!==el&&top!==document.body&&top!==document.documentElement&&n<5){
        if(S.player&&(S.player===top||S.player.contains(top)||top.contains(S.player))) break;
        if(top.contains(el)) break;
        top.setAttribute('data-aot-hide','1'); n++;
        top=document.elementFromPoint(x,y);
      }
    }
  }
  function pickMedia(player){
    if(player.tagName==='IFRAME'||player.tagName==='VIDEO') return player;
    var m=player.querySelectorAll('iframe,video'),best=null,ba=-1,i;
    for(i=0;i<m.length;i++){
      var s=m[i].getAttribute('src')||'';
      if(/^(blob:|javascript:|about:blank)/i.test(s)) continue;
      var a=area(m[i]); if(a>ba){ba=a;best=m[i];}
    }
    return best;
  }
  function apply(){
    var player=(S.player&&document.documentElement.contains(S.player))?S.player:findPlayer();
    if(!player) return false;
    if(!S.title) S.title=pickTitle();
    if(!document.getElementById('aot-style')){
      var st=document.createElement('style'); st.id='aot-style'; st.textContent=css(); (document.head||document.documentElement).appendChild(st);
    }
    var bar=document.getElementById('aot-bar');
    if(!bar){ bar=document.createElement('div'); bar.id='aot-bar'; bar.textContent=S.title; document.body.appendChild(bar); }
    S.player=player;
    markChain(player.parentElement,'aot-keep');
    player.classList.add('aot-player');
    var media=pickMedia(player); S.media=media;
    if(media&&media!==player) media.classList.add('aot-main');
    dockSwitchers(player);
    hideSiblings();
    killOverlays(player);
    unblockSwitchers();
    return true;
  }

  var found=findPlayer();
  if(!found) return 'noplayer';
  S.player=found;
  S.title=pickTitle();
  apply();
  var pr=found.getBoundingClientRect();
  if(!(pr.width>=Math.min(150,window.innerWidth*0.4))){ undo(); return 'noplayer'; }

  var pending=0, misses=0;
  var mo=new MutationObserver(function(){
    if(pending) return;
    pending=setTimeout(function(){
      pending=0;
      try{ if(apply()) misses=0; else if(++misses>=3){ undo(); S.player=null; } }catch(e){}
    },400);
  });
  mo.observe(document.documentElement,{childList:true,subtree:true});
  W.__aotClean={apply:apply,undo:function(){mo.disconnect();undo();W.__aotClean=null;}};
  return 'ok';
}catch(err){
  try{ if(window.__aotClean) window.__aotClean.undo(); else if(undoRef) undoRef(); }catch(e2){}
  return 'error';
}
}"####;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_tolerates_transient_errors_but_stops_when_gone() {
        let mut st = PollState::default();
        let t = Duration::from_secs(1);
        assert_eq!(st.next(EvalOutcome::Error, t), PollAction::Continue);
        assert_eq!(st.next(EvalOutcome::Error, t), PollAction::Continue);
        assert_eq!(st.next(EvalOutcome::Status("\"ok\"".into()), t), PollAction::Continue);
        assert!(st.applied);
        assert_eq!(st.next(EvalOutcome::Error, t), PollAction::Continue);
        assert_eq!(st.next(EvalOutcome::Error, t), PollAction::Continue);
        assert_eq!(st.next(EvalOutcome::Error, t), PollAction::Stop);
    }

    #[test]
    fn poll_gives_up_only_when_a_player_never_appeared() {
        let mut st = PollState::default();
        let np = || EvalOutcome::Status("\"noplayer\"".into());
        assert_eq!(st.next(np(), Duration::from_secs(10)), PollAction::Continue);
        assert_eq!(st.next(np(), Duration::from_secs(130)), PollAction::Stop);
    }

    use scraper::{Html, Selector};
    use std::path::PathBuf;

    fn fixture(site: &str) -> Option<Html> {
        // The animeytx adapter's fixture is named after the domain (animeyt.cc).
        let site = if site == "animeytx" { "animeyt" } else { site };
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("{site}_episode.html"));
        let bytes = std::fs::read(p).ok()?;
        Some(Html::parse_document(&String::from_utf8_lossy(&bytes)))
    }

    fn sel(raw: &str) -> Selector {
        Selector::parse(raw.trim().trim_end_matches('<').trim())
            .unwrap_or_else(|e| panic!("bad selector {raw:?}: {e:?}"))
    }

    #[test]
    fn table_covers_every_site_once() {
        for s in all_sites() {
            let n = SELECTORS.iter().filter(|r| r.site == s.id).count();
            assert_eq!(n, 1, "site {} must have exactly one selector row", s.id);
        }
        for r in SELECTORS {
            assert!(all_sites().iter().any(|s| s.id == r.site), "unknown site {}", r.site);
            assert!(!r.players.is_empty(), "{} has no players", r.site);
        }
    }

    #[test]
    fn all_selectors_parse() {
        for r in SELECTORS {
            for s in r.players.iter().chain(r.switchers.iter()) {
                let _ = sel(s);
            }
            if !r.anchor.is_empty() {
                let _ = sel(r.anchor);
            }
        }
    }

    #[test]
    fn selectors_match_the_episode_fixtures() {
        for r in SELECTORS {
            let Some(doc) = fixture(r.site) else {
                assert!(r.dynamic, "{} is static but has no episode fixture", r.site);
                continue;
            };
            let hits = |list: &[&str]| list.iter().any(|s| doc.select(&sel(s)).next().is_some());
            if r.dynamic {
                if !r.anchor.is_empty() {
                    assert!(doc.select(&sel(r.anchor)).next().is_some(), "{} anchor missing", r.site);
                }
            } else {
                assert!(hits(r.players), "{}: no player selector matches the fixture", r.site);
                // animeytx only renders extra source buttons when an episode has
                // several sources; its fixture has one, so that selector is a guess.
                if !r.switchers.is_empty() && r.site != "animeytx" {
                    assert!(hits(r.switchers), "{}: no switcher selector matches the fixture", r.site);
                }
            }
        }
    }

    #[test]
    fn url_maps_to_its_site_or_to_all() {
        assert_eq!(site_ids_for_url("https://www.tioanime.com/ver/x-1"), vec!["tioanime"]);
        assert_eq!(site_ids_for_url("https://animeland.tv/ep"), vec!["animeland"]);
        assert!(site_ids_for_url("https://some-mirror.example/x").is_empty());
        let all: serde_json::Value = serde_json::from_str(&config_json("https://m.example/x")).unwrap();
        assert!(all["players"].as_array().unwrap().len() > 5);
    }

    #[test]
    fn script_is_one_balanced_expression() {
        let s = script_for("https://tioanime.com/ver/x-1");
        assert!(s.starts_with("(function(cfg){"));
        assert!(s.contains("#video-container"));
        assert_eq!(s.matches('{').count(), s.matches('}').count(), "unbalanced braces");
        assert!(!s.contains("await ") && !s.contains("Promise"), "script must be synchronous");
    }
}
