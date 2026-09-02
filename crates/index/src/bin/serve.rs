//! The pass-index container's HTTP face.
//!
//! Two things are served here and they answer different readers. `/index` is
//! the browser: one page carrying the whole catalogue, rendered fresh because
//! a cached copy of it is a stale catalogue. Everything under `/index/…` is a
//! page per thing — a company, a product, a board, a task, a licence — small
//! enough to be read by a crawler that will not run JavaScript, addressed by
//! what the thing is called rather than by the id a feed happened to mint.
//!
//! Every page has a JSON twin at the same address plus `.json`, which is what
//! an agent should read and what saves the browser from being handed five
//! megabytes to show one model.

use axum::{
    extract::{Path, Request},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const BROWSE: &str = include_str!("../../web/browse.html");
/// Style v2, laid over the live stylesheet rather than replacing it, so the
/// two can be compared on the same page by adding ?v2 to any address.
const CSS_V2: &str = include_str!("../../web/page-v2.css");
const STYLE: &str = include_str!("../../web/page.css");
/// Three states, because "follow the system" is a real answer and the reader
/// who has never chosen should not be forced to. Inline on every page so the
/// chosen theme is applied before the first paint rather than after it.
/// Sorting a seller table, for a page that lists twenty of them. By rate and
/// by name only: those are the two questions a reader has, and a table that
/// sorts by everything sorts by nothing.
const SIZES_JS: &str = r##"(function(){
// Two controls that do different jobs, and the page has to keep them apart.
// The band is a filter — which models are shown at all — and the sort is an
// order. Choosing a band must not throw away the order the reader picked.
var rows=[].slice.call(document.querySelectorAll("#srows li"));
var band="all", by="s", desc=true;
function num(li,k){return parseFloat(li.dataset[k]||"0")||0}
function apply(){
 var live=rows.filter(function(li){return band==="all"||li.dataset.band===band});
 live.sort(function(a,b){
   // Names read the other way round, like price: the first click on a
   // column gives the direction somebody actually wants — most sellers,
   // most boards, biggest, cheapest, and A before Z.
   if(by==="n"){var x=a.dataset.n,y=b.dataset.n;return desc?x.localeCompare(y):y.localeCompare(x)}
   var d=num(a,by)-num(b,by);
   // Price reads the other way round: cheapest first is what a reader wants.
   if(by==="p") d=-d;
   return desc? -d : d;
 });
 var l=document.getElementById("srows");
 rows.forEach(function(li){li.style.display="none"});
 live.forEach(function(li){li.style.display="";l.appendChild(li)});
 document.getElementById("scount").textContent=
   live.length.toLocaleString("en")+" of "+rows.length.toLocaleString("en")+" models";
}
document.getElementById("bands").addEventListener("click",function(e){
 var b=e.target.closest("button"); if(!b||b.disabled)return;
 band=b.dataset.band;
 [].forEach.call(this.children,function(x){x.classList.toggle("on",x===b)});
 apply();
});
document.getElementById("ssort").addEventListener("click",function(e){
 var b=e.target.closest("button"); if(!b)return;
 if(b.dataset.by===by){desc=!desc}else{by=b.dataset.by;desc=true}
 [].forEach.call(this.children,function(x){x.classList.toggle("on",x===b)});
 apply();
});
apply();
})();"##;

const SORT_JS: &str = r#"document.addEventListener("click",function(e){
var h=e.target.closest("tr.agg-head");
if(h){h.classList.toggle("open");
 var g=h.dataset.g,b=h.closest("tbody");
 [].forEach.call(b.querySelectorAll('tr.agg[data-g="'+g+'"]'),function(r){
   r.classList.toggle("shown",h.classList.contains("open"))});
 return}
var b=e.target.closest(".sortable th button"); if(!b)return;
var t=b.closest("table"),by=b.dataset.by,wasAsc=b.dataset.desc!=="1",
    asc=b.classList.contains("on")?!wasAsc:true;
b.dataset.desc=asc?"0":"1";
t.querySelectorAll("th button").forEach(function(x){x.classList.toggle("on",x===b);
 if(x!==b)delete x.dataset.desc});
var body=t.tBodies[0],rows=[].slice.call(body.rows);
rows.sort(function(x,y){var a,c;
 if(by==="p"||by==="s"||by==="b"){a=+x.dataset[by];c=+y.dataset[by]}
 else{a=x.dataset.n;c=y.dataset.n}
 return (a>c?1:a<c?-1:0)*(asc?1:-1)});
rows.forEach(function(r){body.appendChild(r)});
b.setAttribute("aria-sort",asc?"ascending":"descending")});
"#;

const THEME_JS: &str = r#"(function(){var M=["auto","light","dark"],
D={auto:"M12 3a9 9 0 100 18 9 9 0 000-18zm0 0v18",light:"M12 4v2m0 12v2M4 12H2m20 0h-2M6 6L4.5 4.5M18 6l1.5-1.5M6 18l-1.5 1.5M18 18l1.5 1.5M16 12a4 4 0 11-8 0 4 4 0 018 0z",
dark:"M20 14.5A8.5 8.5 0 019.5 4a8.5 8.5 0 1010.5 10.5z"},
k="pass-index-theme",t=localStorage.getItem(k)||"auto";
function put(v){t=v;localStorage.setItem(k,v);
 if(v==="auto"){document.documentElement.removeAttribute("data-theme")}
 else{document.documentElement.setAttribute("data-theme",v)}
 var i=document.getElementById("ti"),b=document.getElementById("theme");
 if(i){i.setAttribute("d",D[v])} if(b){b.setAttribute("aria-label","Theme: "+v)}}
put(t);
document.addEventListener("DOMContentLoaded",function(){put(t);
 var b=document.getElementById("theme");
 if(b){b.addEventListener("click",function(){put(M[(M.indexOf(t)+1)%3])})}});})();"#;

fn db() -> String {
    std::env::var("PASS_INDEX_DB").unwrap_or_else(|_| "/data/index.db".into())
}

fn open() -> anyhow::Result<index::Index> {
    index::Index::open(&db())
}

/// The holding pen, which lives in its own file next to the catalogue. Every
/// other handler on this server opens `index.db`; this is the only one that
/// knows the other database exists, which is the point of it being a file
/// rather than a column.
fn pen() -> String {
    std::env::var("PASS_INDEX_PEN")
        .unwrap_or_else(|_| db().replace("index.db", "quarantine.db"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// An href for a URL that came from outside data (a crawler wire line, a
/// seller's own site, a board's page). Attribute-escaping alone does not stop
/// `javascript:` or `data:` — a scheme the browser will execute in this
/// origin — so a URL that is not plainly http(s) or root-relative becomes an
/// inert anchor. Only our own paths and real web links survive as links.
fn safe_href(url: &str) -> String {
    let u = url.trim();
    // A single leading slash is one of our own paths; "//host" is a
    // protocol-relative link offsite, so it must not count as root-relative.
    let ok = (u.starts_with('/') && !u.starts_with("//"))
        || u.len() >= 7 && u[..7].eq_ignore_ascii_case("http://")
        || u.len() >= 8 && u[..8].eq_ignore_ascii_case("https://");
    if ok { esc(u) } else { String::from("#") }
}

fn html(body: String) -> axum::response::Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // One writer, once a night. A reader may hold a page for an hour
            // and still be looking at what the catalogue holds.
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        body,
    )
        .into_response()
}

/// The browsing page is a program, not a document: its script and the search
/// index it fetches have to be the same age. Cached for an hour they drift —
/// yesterday's script filtering on a register yesterday's index did not
/// carry, which is an empty list and no error anywhere. It is the second time
/// a stale shell has looked exactly like a missing feature.
fn app(body: String) -> axum::response::Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// The search index the browsing page runs on. Revalidated rather than held,
/// for the same reason: it must never be older than the script reading it.
fn live_json(v: Value) -> axum::response::Response {
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        v.to_string(),
    )
        .into_response()
}

fn json(v: Value) -> axum::response::Response {
    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        v.to_string(),
    )
        .into_response()
}

fn missing() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        shell(
            "Not in the catalogue",
            "",
            "/index",
            "<h1>Not in the catalogue</h1><p class=\"lede\">No company, product, \
             board, task or licence answers on this address. \
             <a href=\"/index\">Browse what is here</a>.</p>"
                .into(),
            String::new(),
        ),
    )
        .into_response()
}

/// The page around the page: one stylesheet, one canonical, one line for a
/// search engine to quote, and the structured record for one to parse.
/// Google Analytics. The only third-party request any page makes: everything
/// else — the stylesheet, the icons, the scripts — is inlined so a page is one
/// round trip. Loaded async and written last, so nothing a reader came for
/// waits on somebody else's server.
const ANALYTICS: &str = r#"<script async src="https://www.googletagmanager.com/gtag/js?id=G-QS8QHPRGJS"></script>
<script>window.dataLayer=window.dataLayer||[];function gtag(){dataLayer.push(arguments)}
gtag('js',new Date());gtag('config','G-QS8QHPRGJS');</script>"#;

fn shell(title: &str, lede: &str, href: &str, body: String, jsonld: String) -> String {
    shell_with(title, lede, href, body, jsonld, false)
}

/// `hidden` keeps a page out of every index that would carry it onwards. The
/// pen is for a person doing the checking, not for a reader who arrived from
/// a search result expecting a catalogue entry.
fn shell_with(
    title: &str,
    lede: &str,
    href: &str,
    body: String,
    jsonld: String,
    hidden: bool,
) -> String {
    let robots = if hidden {
        "<meta name=\"robots\" content=\"noindex, nofollow\">\n"
    } else {
        ""
    };
    format!(
        r#"<!doctype html><html lang="en"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="format-detection" content="telephone=no,date=no">
<title>{t} · Pass Index</title>
<meta name="description" content="{d}">
{robots}<link rel="canonical" href="https://pass.io{h}">
<meta property="og:title" content="{t} · Pass Index">
<meta property="og:description" content="{d}">
<meta property="og:url" content="https://pass.io{h}">
<style>{css}</style>
<style id="v2" media="all">{v2}</style>
<script>var q=new URLSearchParams(location.search),e=document.getElementById("v2");
if(q.has("v1")){{e.media="not all";localStorage.setItem("style","v1")}}
else if(q.has("v2")){{localStorage.removeItem("style")}}
else if(localStorage.getItem("style")==="v1"){{e.media="not all"}}</script>
{ld}</head><body>
<header><a class="brand" href="/index">Pass Index</a><span class="sub">The State of AI</span><a class="cta" href="/signin?signin={h}">Sign in</a></header>
<main>{body}</main>
<footer><a class="home" href="/">Pass Index</a>
 · <a href="/index/top">Top</a>
 · <a href="/index/free">Free</a>
 · <a href="/index/sizes">Sizes</a>
 · <a href="/index/lists">Lists</a>
 · <a href="/index/providers">Companies</a>
 · <a href="/index/models">Models</a>
 · <a href="/index/tools">Tools</a>
 · <a href="/index/agents">Agents</a>
 · <a href="/index/subscriptions">Subscriptions</a>
 · <a href="/index/waiting">B2B</a>
 · <a href="/index/coverage">Status</a>
<button id="theme" type="button" aria-label="Switch theme"><svg viewBox="0 0 24 24"
 aria-hidden="true"><path id="ti" d=""/></svg></button></footer>
<script>{theme}{sorter}{copy}</script>{tag}</body></html>"#,
        robots = robots,
        t = esc(title),
        d = esc(lede),
        h = esc(href),
        css = STYLE,
        v2 = CSS_V2,
        theme = THEME_JS,
        copy = COPY_JS,
        tag = ANALYTICS,
        sorter = SORT_JS,
        ld = if jsonld.is_empty() {
            String::new()
        } else {
            // serde_json escapes quotes and backslashes but not "</script>",
            // and the strings inside came from third-party feeds: a listing
            // whose name carried that tag closed the block and ran whatever
            // followed as page script. The HTML spec's own answer is to
            // escape the guillemets at the JSON level.
            let safe = jsonld.replace("</", "\\u003C/");
            format!("<script type=\"application/ld+json\">{safe}</script>\n")
        },
        body = body
    )
}

fn money(micros: i64) -> String {
    index::money(micros)
}

/// Kept only for the order the card leads in; the words themselves come from
/// `index::unit_label`, so the tables, the prose and the picks cannot drift.
fn unit(dim: &str) -> String {
    let w = index::unit_label(dim);
    if w.is_empty() { format!("per {dim}") } else { w.to_string() }
}

/// Which figure leads, when a thing is metered several ways at once.
const LEAD_ORDER: &[&str] = &[
    // A thing sold by the month is sold by the month; that is the figure the
    // buyer signs for, so it leads whatever else it is also metered by.
    "month",
    "mtok_in", "mtok_out", "image", "second", "minute", "call", "character", "result", "page",
];

/// Thirteen thousand four hundred and one reads as a number; 13401 reads as
/// a serial. Anything a person is meant to take in gets its separators.
/// A parameter count as a reader says it: "70B", "1.6T".
fn billions(params: i64) -> String {
    let b = params as f64 / 1e9;
    if b >= 1000.0 {
        format!("{:.1}T", b / 1000.0)
    } else if b >= 10.0 {
        format!("{b:.0}B")
    } else {
        format!("{}B", format!("{b:.1}").trim_end_matches('0').trim_end_matches('.'))
    }
}

fn grouped(n: i64) -> String {
    index::grouped(n)
}

/// A score as the board prints it: no trailing zeros invented by formatting.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

const PER_PAGE: usize = 100;

/// One row of a list, wherever the list is. The browsing page and the hubs
/// were drawing the same thing two ways, and only one of them had prices.
fn row_html(x: &Value) -> String {
    let price = match (x["p"].as_i64(), x["d"].as_str()) {
        (Some(p), Some(d)) => {
            let (figure, u) = match (x["o"].as_i64(), x["x"].as_i64()) {
                (Some(out), _) => (
                    format!("{}<span class=\"to\">→</span>{}", money(p), money(out)),
                    "per Mtok in / out".to_string(),
                ),
                (None, Some(hi)) if hi > p => (
                    format!("{}<span class=\"to\">–</span>{}", money(p), money(hi)),
                    unit(d),
                ),
                _ => (money(p), unit(d)),
            };
            format!("<span class=\"pr\">{figure}<span class=\"un\">{}</span></span>", esc(&u))
        }
        _ => String::new(),
    };
    // Where it has placed best, beside how many sell it. The catalogue holds
    // both and the row was showing only the second, which said what a thing
    // costs without saying whether it is any good.
    let stand = match (x["br"].as_i64(), x["bf"].as_i64()) {
        (Some(r), Some(f)) if f > 1 => {
            let boards = match x["bn"].as_i64().unwrap_or(0) {
                n if n > 1 => format!("<i>· {n} boards</i>"),
                _ => String::new(),
            };
            format!("<span class=\"st\">{} of {}{boards}</span>", ordinal(r), grouped(f))
        }
        _ => String::new(),
    };
    // A plan's own company selling it is not a fact worth a column; what the
    // plan allows is. So on a subscription the terms take the seller count's
    // place, and everywhere else nothing changes.
    let sellers = match (x["lm"].as_str(), x["s"].as_i64().unwrap_or(0)) {
        (Some(l), _) if !l.is_empty() => format!("<span class=\"sc terms\">{}</span>", esc(l)),
        (_, 0) => String::new(),
        (_, n) => format!(
            "<span class=\"sc\">{n} {}</span>",
            if x["r"] == "provider" { "sold" } else { "selling" }
        ),
    };
    format!(
        "<li><a href=\"{}\"><span class=\"nm\">{}</span>\
         <span class=\"mk\">{}{stand}</span>{price}{sellers}</a></li>",
        esc(x["h"].as_str().unwrap_or("/index")),
        esc(x["n"].as_str().unwrap_or("")),
        esc(x["m"].as_str().or(x["k"].as_str()).unwrap_or("")),
    )
}

/// A tag is a filing word; a sentence needs English. "for speak" is neither.
const TASK_WORDS: &[(&str, &str)] = &[
    ("chat", "conversation"),
    ("agents", "agentic"),
    ("reasoning", "reasoning"),
    ("code", "writing code"),
    ("embedding", "embeddings"),
    ("rerank", "reranking search results"),
    ("ocr", "reading documents"),
    ("transcribe", "transcription"),
    ("speak", "speech"),
    ("music", "music"),
    ("translate", "translation"),
    ("guard", "safety classification"),
    ("search", "search"),
    ("crawl", "crawling the web"),
    ("extract", "structured extraction"),
    ("sandbox", "running code"),
    ("evaluate", "evaluation"),
    ("image", "images"),
    ("video", "video"),
    ("avatar", "avatar video"),
];

fn task_words(tag: &str) -> String {
    TASK_WORDS
        .iter()
        .find(|(t, _)| *t == tag)
        .map(|(_, w)| w.to_string())
        .unwrap_or_else(|| tag.to_string())
}

/// "a agent" is the sort of thing a reader notices and a writer never does.
fn article(word: &str) -> &'static str {
    match word.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('a') | Some('e') | Some('i') | Some('o') | Some('u') => "an",
        _ => "a",
    }
}

fn ordinal(n: i64) -> String {
    index::ordinal(n)
}

// ---- the product page ------------------------------------------------------

fn entity_body(ix: &index::Index, e: &Value) -> anyhow::Result<(String, String, String)> {
    let name = e["name"].as_str().unwrap_or_default();
    let maker_id = e["maker"].as_str().unwrap_or_default();
    let makers: Vec<(String, String, String)> = ix.provider_addresses()?;
    let maker = makers.iter().find(|(id, _, _)| id == maker_id);
    let attrs = &e["attrs"];
    let register = e["register"].as_str().unwrap_or("model");
    // A subscription has no modality: nothing goes in and nothing comes out of
    // a plan. Printed anyway it produced a bare "→ ·" at the top of the card.
    // What a plan does have is its allowance, and that is the half of its
    // price a monthly figure cannot carry.
    let takes = e["input_kind"].as_str().unwrap_or("");
    let gives = e["output_kind"].as_str().unwrap_or("");
    let io = if takes.is_empty() && gives.is_empty() {
        attrs["limits"]
            .as_str()
            .map(|l| l.to_string())
            .unwrap_or_default()
    } else {
        format!("{takes} → {gives}")
    };

    let lede = e["docs"]
        .as_array()
        .and_then(|d| {
            d.iter()
                .find(|x| x["kind"] == "description")
                .and_then(|x| x["text"].as_str())
        })
        .map(|t| t.split(". ").next().unwrap_or(t).trim_end_matches('.').to_string() + ".")
        .unwrap_or_else(|| {
            // 143 things have no sentence from their maker yet, and this line
            // is what a search engine will quote. Say what is actually known
            // rather than repeating the modality printed directly below it.
            let does = e["attrs"]["tasks"]
                .as_array()
                .map(|t| {
                    t.iter()
                        .filter_map(|x| x.as_str())
                        .map(task_words)
                        .collect::<Vec<_>>()
                        .join(" and ")
                })
                .filter(|s| !s.is_empty());
            let made = match maker {
                Some((_, mname, _)) => format!(" by {mname}"),
                None => String::new(),
            };
            let opening = match does {
                Some(d) => format!("{name} is {} {register}{made} for {d}", article(register)),
                None => format!("{name} is {} {register}{made}", article(register)),
            };
            let sellers = e["offerings"]
                .as_array()
                .map(|o| {
                    o.iter()
                        .filter_map(|x| x["provider"].as_str())
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                })
                .unwrap_or(0);
            // A thing sold only on subscription has no seller of units, which
            // is not the same as nobody publishing a price for it.
            let plan_count = ix.plans_for(e["id"].as_str().unwrap_or("")).map(|p| p.len()).unwrap_or(0);
            let sold = match sellers {
                0 if plan_count > 0 => format!(
                    "sold by the month on {}",
                    if plan_count == 1 { "one plan".to_string() } else { format!("{plan_count} plans") }
                ),
                0 => "no seller in the catalogue publishes a price for it".to_string(),
                1 => "one seller in the catalogue publishes a price for it".to_string(),
                n => format!("{n} sellers in the catalogue publish a price for it"),
            };
            let ranked = e["benchmarks"]
                .as_array()
                .and_then(|b| b.iter().min_by_key(|x| x["rank"].as_i64().unwrap_or(i64::MAX)))
                .and_then(|b| {
                    Some(format!(
                        ", it stands {} of {} on {}",
                        ordinal(b["rank"].as_i64()?),
                        b["out_of"].as_i64()?,
                        b["suite_name"]
                            .as_str()
                            .unwrap_or(b["suite"].as_str()?)
                            .split(" (")
                            .next()
                            .unwrap_or_default()
                    ))
                })
                .unwrap_or_default();
            format!("{opening}{ranked}, and {sold}.")
        });

    let mut body = format!(
        "<h1>{}</h1><p class=\"lede\">{}</p><p class=\"io\">{} · {}</p>",
        esc(name),
        esc(&lede),
        esc(&io),
        match maker {
            Some((_, mname, mslug)) =>
                format!("made by <a href=\"/index/{mslug}\">{}</a>", esc(mname)),
            None => "no single maker — the market carries it".into(),
        }
    );

    // what it is for, and what you may do with it
    let mut chips = String::new();
    if let Some(tasks) = attrs["tasks"].as_array() {
        for t in tasks {
            if let Some(t) = t.as_str() {
                chips.push_str(&format!(
                    "<a class=\"chip\" href=\"/index/for/{t}\">{}</a>",
                    esc(t)
                ));
            }
        }
    }
    if let Some(l) = attrs["license"].as_str() {
        chips.push_str(&format!(
            "<a class=\"chip\" href=\"/index/licence/{}\">{}</a>",
            index::address_slug(l),
            esc(l)
        ));
    }
    if let Some(c) = attrs["context"].as_i64() {
        chips.push_str(&format!("<span class=\"chip flat\">{} context</span>", grouped(c)));
    }

    // What it is, in our own words and only from what the catalogue can show
    // a source for. The composer wants the maker's name rather than its id.
    let mut with_maker = e.clone();
    if let Some((_, mname, _)) = maker {
        with_maker["maker_name"] = Value::String(mname.clone());
    }
    with_maker["plans"] = Value::Array(ix.plans_for(e["id"].as_str().unwrap_or("")).unwrap_or_default());
    let about = index::about::entity(&with_maker);
    // Who sells it and for how much. The fact worth leading with is the
    // spread: the same weights cost four and a half times more from the
    // dearest seller than the cheapest, and a table of seventy-five rows
    // hides that. So the range is drawn once at the top, and every row shows
    // where it sits inside it.
    let empty = vec![];
    let offerings = e["offerings"].as_array().unwrap_or(&empty);
    // Which unit this card is quoted in, and the same one the About panel
    // uses — the unit most of its sellers meter it by, with the fixed order
    // only to break a tie. Two rules for one question put "$0.0001 per Mtok"
    // in Whisper's headline and "$0.0005 a minute" three inches below it,
    // and a reader cannot be expected to know which of us is right.
    let quoted_by = |d: &str| -> usize {
        offerings
            .iter()
            .filter(|o| {
                o["components"]
                    .as_array()
                    .map(|cs| cs.iter().any(|c| c["dimension"] == d))
                    .unwrap_or(false)
            })
            .count()
    };
    let lead_dim = LEAD_ORDER
        .iter()
        .filter(|d| quoted_by(d) > 0)
        .max_by_key(|d| {
            (
                if ***d == *"month" { usize::MAX } else { quoted_by(d) },
                std::cmp::Reverse(LEAD_ORDER.iter().position(|x| x == *d).unwrap_or(99)),
            )
        })
        .copied();
    let rate_of = |o: &Value, dim: &str| -> Option<i64> {
        o["components"]
            .as_array()?
            .iter()
            .find(|c| c["dimension"] == dim)
            .and_then(|c| c["micros_per_unit"].as_i64())
    };
    // A batch queue is a different product from an answer now, and quoting
    // its rate as the price of the model is how a reader ends up budgeting
    // against a number that does not exist for what they are about to do.
    // The spread is over the standard lane; the other lanes are in the table.
    // Standard lane AND still live: the headline must not quote a rate from a
    // shelved offering. The list surfaces (find_index, dollar_models,
    // sized_models) all filter status='live'; the card left it out, so one
    // model showed a stale seller's price on its card and the live price in
    // the lists.
    let standard = |o: &Value| {
        o["variant"].as_str().unwrap_or("").is_empty()
            && o["status"].as_str().unwrap_or("live") == "live"
    };
    let mut spread: Vec<(i64, String)> = Vec::new();
    if let Some(dim) = lead_dim {
        for o in offerings.iter().filter(|o| standard(o)) {
            if let Some(m) = rate_of(o, dim) {
                spread.push((m, o["provider"].as_str().unwrap_or("").to_string()));
            }
        }
        if spread.is_empty() {
            for o in offerings {
                if let Some(m) = rate_of(o, dim) {
                    spread.push((m, o["provider"].as_str().unwrap_or("").to_string()));
                }
            }
        }
        spread.sort();
    }

    if let (Some(dim), Some((low, who))) = (lead_dim, spread.first().cloned()) {
        let high = spread.last().map(|(m, _)| *m).unwrap_or(low);
        // the pair a reader quotes: what goes in, what comes back
        let paired = offerings
            .iter()
            .filter(|o| standard(o) || spread.len() == offerings.len())
            .find(|o| rate_of(o, dim) == Some(low))
            .and_then(|o| match dim {
                "mtok_in" => rate_of(o, "mtok_out").map(|out| (low, out)),
                _ => None,
            });
        let big = match paired {
            Some((a, b)) => format!(
                "{}<span class=\"arrow\">→</span>{}",
                money(a),
                money(b)
            ),
            None => money(low),
        };
        let unit_text = match dim {
            "mtok_in" if paired.is_some() => "per Mtok in / out".to_string(),
            d => unit(d),
        };
        let sellers = offerings
            .iter()
            .filter_map(|o| o["provider"].as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let said = format!(
            "<b>{}</b>{}",
            esc(&who),
            if sellers > 1 {
                format!(" · {} sellers", sellers)
            } else {
                String::new()
            }
        );
        body.push_str(&format!(
            "<div class=\"hero\"><div class=\"big\">{big}<span class=\"unit\">{}</span></div>\
             <div class=\"said\">{said}</div></div>",
            esc(&unit_text)
        ));
    }


    // Where the same thing is handed out for nothing. Deliberately its own
    // block and deliberately after the price: a free lane is not a cheaper
    // rate, it is a different arrangement with a cap on it, and a reader who
    // budgets against it will be wrong.
    let free: Vec<&Value> = e["offerings"]
        .as_array()
        .map(|v| v.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter(|o| o["variant"].as_str() == Some("free"))
        .collect();
    if !free.is_empty() {
        let rows: String = free
            .iter()
            .map(|o| {
                let terms = o["limits"].as_str().unwrap_or("");
                format!(
                    "<li><span class=\"who\">{}</span>{}</li>",
                    esc(o["provider"].as_str().unwrap_or("")),
                    if terms.is_empty() || terms == "no end date given" {
                        String::new()
                    } else {
                        format!("<span class=\"terms\">{}</span>", esc(terms))
                    }
                )
            })
            .collect();
        body.push_str(&format!(
            "<section class=\"gratis\"><h2>Free</h2>\
             <p class=\"intro\">Offered at no charge, which is not the same as cheap: a \
              free lane carries a rate limit and can be withdrawn. The prices above are \
              what you pay when it is not available to you.</p>\
             <ul class=\"who\">{rows}</ul></section>"
        ));
    }

    // What it costs by the month. A thing sold only on subscription has no
    // rate of its own, and its card would otherwise never say a price.
    let plans = ix.plans_for(e["id"].as_str().unwrap_or("")).unwrap_or_default();
    if !plans.is_empty() {
        let rows: String = plans
            .iter()
            .map(|p| {
                let limits = p["limits"].as_str().unwrap_or("");
                format!(
                    "<li><a href=\"{}\"><span class=\"nm\">{}</span>\
                     <span class=\"mk\">{}{}</span>\
                     <span class=\"pr\">{}<span class=\"un\">a month</span></span></a></li>",
                    esc(p["href"].as_str().unwrap_or("#")),
                    esc(p["name"].as_str().unwrap_or("")),
                    esc(p["seller"].as_str().unwrap_or("")),
                    if limits.is_empty() {
                        String::new()
                    } else {
                        format!("<span class=\"st\">{}</span>", esc(limits))
                    },
                    money(p["month"].as_i64().unwrap_or(0))
                )
            })
            .collect();
        body.push_str(&format!(
            "<h2>Available within a subscription <span class=\"n\">{} plan{}</span></h2>\
             <p class=\"intro\">Bought by the month. What a plan allows is the half of its \
              price that a rate card cannot carry, so it is printed beside every one.</p>\
             <ul class=\"rows plain\">{rows}</ul>",
            plans.len(),
            if plans.len() == 1 { "" } else { "s" }
        ));
    }


    // The standard lanes first and the cheapest at the top: the table should
    // open on the answer, not on whichever seller the feed happened to list.
    let mut ordered: Vec<&Value> = offerings.iter().collect();
    ordered.sort_by_key(|o| {
        (
            lead_dim.and_then(|d| rate_of(o, d)).unwrap_or(i64::MAX),
            !standard(o),
            o["provider"].as_str().unwrap_or("").to_string(),
        )
    });
    // One entry per seller, in one queue from the low price to the high one.
    // An aggregator reselling the same model on four routes is one seller
    // holding one place in that queue; its routes open underneath it.
    struct Entry {
        low: i64,
        high: i64,
        name: String,
        ways: usize,
        rows: String,
        agg: bool,
        // The figures of this seller's cheapest lane, so a folded line quotes
        // the same in-and-out pair every other line quotes.
        head: String,
        paired: bool,
        // Whether the figure above came off a standard lane. A batch or
        // priority rate is a different product, not a cheaper price.
        plain: bool,
    }
    let mut entries: std::collections::BTreeMap<String, Entry> =
        std::collections::BTreeMap::new();
    for o in ordered {
        // The pair a reader quotes comes first; the cache rates are a footnote
        // to it, and the database's alphabetical order put them in front.
        let mut cs: Vec<&Value> = o["components"].as_array().unwrap_or(&empty).iter().collect();
        cs.sort_by_key(|c| {
            let d = c["dimension"].as_str().unwrap_or("");
            (
                LEAD_ORDER.iter().position(|x| *x == d).unwrap_or(LEAD_ORDER.len()),
                d.to_string(),
            )
        });
        let comps: Vec<String> = cs
            .into_iter()
            .map(|c| {
                let m = c["micros_per_unit"].as_i64().unwrap_or(0);
                let d = c["dimension"].as_str().unwrap_or("");
                format!(
                    "<span class=\"fig\"><b>{}</b><span class=\"u\">{}</span></span>",
                    money(m),
                    esc(&unit(d))
                )
            })
            .collect();
        // how far along the range this seller sits, when there is a range
        let sit = match (lead_dim, spread.first(), spread.last()) {
            (Some(dim), Some((low, _)), Some((high, _))) if high > low => rate_of(o, dim)
                .map(|m| {
                    let pct = ((m - low) as f64 / (high - low) as f64 * 100.0).clamp(0.0, 100.0);
                    format!("<span class=\"sit\"><i style=\"left:{pct:.0}%\"></i></span>")
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        let is_cheapest = standard(o)
            && matches!((lead_dim, spread.first()),
                        (Some(dim), Some((low, _))) if rate_of(o, dim) == Some(*low));
        let sort_key = lead_dim.and_then(|d| rate_of(o, d)).unwrap_or(i64::MAX);
        // An aggregator is reselling somebody else's model, and nine of them
        // quoting near-identical rates push the companies that actually run
        // the thing off the screen. They fold into one line, cheapest first,
        // and open when a reader wants them.
        let is_agg = o["way"].as_str() == Some("aggregator");
        let seller = o["provider"].as_str().unwrap_or("").to_string();
        let slug = index::address_slug(&seller);
        let row = format!(
            "<tr class=\"{}{}\" data-g=\"{slug}\" data-p=\"{sort_key}\" data-n=\"{}\">\
             <td class=\"{}\">{} <span class=\"tag\">{}</span></td>\
             <td>{}</td><td class=\"figs\">{}{sit}</td></tr>",
            "",
            if is_cheapest { "cheap-row" } else { "" },
            esc(&o["provider"].as_str().unwrap_or("").to_lowercase()),
            if is_cheapest { "cheapest" } else { "" },
            esc(o["provider"].as_str().unwrap_or("")),
            esc(o["way"].as_str().unwrap_or("")),
            esc(match o["variant"].as_str() {
                Some("") | None => "standard",
                Some(v) => v,
            }),
            comps.join("")
        );
        // A direct seller keeps its own place per lane; an aggregator's lanes
        // collect under its name.
        let key = if is_agg {
            format!("a{slug}")
        } else {
            format!("d{slug}{sort_key:020}{}", esc(o["variant"].as_str().unwrap_or("")))
        };
        let e = entries.entry(key).or_insert_with(|| Entry {
            low: i64::MAX,
            high: 0,
            name: seller.clone(),
            ways: 0,
            rows: String::new(),
            agg: is_agg,
            head: String::new(),
            paired: false,
            plain: false,
        });
        let plain = standard(o);
        if sort_key != i64::MAX && (plain && !e.plain || plain == e.plain && sort_key < e.low) {
            e.head = comps.join("");
            e.paired = rate_of(o, "mtok_in").is_some() && rate_of(o, "mtok_out").is_some();
            e.plain = plain;
        }
        if sort_key != i64::MAX && (plain || !e.plain) {
            if plain && !e.plain {
                e.low = sort_key;
                e.high = sort_key;
                e.plain = true;
            } else {
                e.low = e.low.min(sort_key);
                e.high = e.high.max(sort_key);
            }
        }
        e.ways += 1;
        e.rows.push_str(&row);
    }
    let mut queue: Vec<Entry> = entries.into_values().collect();
    queue.sort_by(|a, b| {
        a.agg
            .cmp(&b.agg)
            .then(a.low.cmp(&b.low))
            .then(a.name.cmp(&b.name))
    });
    let mut rows = String::new();
    for e in &queue {
        // One way to buy needs no fold: the line would open onto itself.
        if e.ways == 1 {
            rows.push_str(&e.rows);
            continue;
        }
        // Metered by tokens: quote the pair, as every other line does. Metered
        // by the call or the request: no pair exists, so the span from its
        // cheapest route to its dearest is the honest figure.
        let figure = if e.paired {
            e.head.clone()
        } else if e.high > e.low {
            format!(
                "<span class=\"fig\"><b>{}</b><span class=\"to\">–</span><b>{}</b></span>",
                money(e.low),
                money(e.high)
            )
        } else if e.low != i64::MAX {
            format!("<span class=\"fig\"><b>{}</b></span>", money(e.low))
        } else {
            // The sentinel for "no rate we could read". Printed through
            // money() it became $9,223,372,036,855 on a card.
            String::new()
        };
        let sit = match (spread.first(), spread.last()) {
            (Some((low, _)), Some((high, _))) if high > low && e.low != i64::MAX => {
                let pct = ((e.low - low) as f64 / (high - low) as f64 * 100.0).clamp(0.0, 100.0);
                format!("<span class=\"sit\"><i style=\"left:{pct:.0}%\"></i></span>")
            }
            _ => String::new(),
        };
        rows.push_str(&format!(
            "<tr class=\"agg-head\" data-g=\"{}\"><td>{} \
             <span class=\"tag\">{} ways</span></td><td></td>\
             <td class=\"figs\">{figure}{sit}</td></tr>",
            index::address_slug(&e.name),
            esc(&e.name),
            e.ways
        ));
        rows.push_str(&e.rows.replace("<tr class=\"", "<tr class=\"agg "));
    }
    if !rows.is_empty() {
        body.push_str(&format!(
            "<h2>Sold by <span class=\"n\">{} way{}</span></h2><div class=\"scroll\">\
             <table class=\"grid\"><thead><tr>\
             <th>Seller</th><th>Lane</th><th>Rate</th>\
             </tr></thead><tbody>{rows}</tbody></table></div>",
            offerings.len(),
            if offerings.len() == 1 { "" } else { "s" }
        ));
    } else {
        body.push_str("<h2>Sold by</h2><p class=\"none\">Nobody in the catalogue publishes a price for this yet.</p>");
    }
    // where it stands
    let boards = e["benchmarks"].as_array().unwrap_or(&empty);
    if !boards.is_empty() {
        let mut brows = String::new();
        for b in boards {
            let suite = b["suite"].as_str().unwrap_or("");
            let (rank, out_of) = (b["rank"].as_i64(), b["out_of"].as_i64());
            let place = match (rank, out_of) {
                (Some(r), Some(n)) => format!(
                    "{}<span class=\"of\">of {n}</span>",
                    ordinal(r)
                ),
                (Some(r), None) => ordinal(r),
                _ => "—".into(),
            };
            // how much of the field is below this row: a third of five and a
            // third of five hundred are not the same result
            let field = match (rank, out_of) {
                (Some(r), Some(n)) if n > 1 => {
                    let pct = ((n - r) as f64 / (n - 1) as f64 * 100.0).clamp(2.0, 100.0);
                    format!("<span class=\"field\"><i style=\"width:{pct:.0}%\"></i></span>")
                }
                _ => String::new(),
            };
            brows.push_str(&format!(
                "<tr><td class=\"place{}\">{}</td><td><a href=\"/index/board/{}\">{}</a>{field}</td>\
                 <td>{}</td><td class=\"score\">{}</td></tr>",
                if rank == Some(1) { " top" } else { "" },
                place,
                index::address_slug(suite),
                esc(b["suite_name"].as_str().unwrap_or(suite)),
                esc(b["metric"].as_str().unwrap_or("")),
                num(b["value"].as_f64().unwrap_or(0.0))
            ));
        }
        body.push_str(&format!(
            "<h2>Measured <span class=\"n\">{} standing{}</span></h2><div class=\"scroll\">\
             <table class=\"grid rank\"><thead><tr><th>Place</th><th>Board</th>\
             <th>Metric</th><th>Score</th></tr></thead><tbody>{brows}</tbody></table></div>",
            boards.len(),
            if boards.len() == 1 { "" } else { "s" }
        ));
    }

    body.push_str(&about_html(
        &about,
        &ix.last_read().unwrap_or_default(),
        &format!(
            "/index/{}",
            maker
                .map(|(_, _, slug)| slug.clone())
                .unwrap_or_else(|| "commons".into())
        ),
        name,
    ));

    // what sellers call it
    // What sellers call it — where that differs from what it is called. An
    // alias identical to the name teaches a reader nothing, and 524 of them
    // were filling this block.
    let mut seen = std::collections::BTreeSet::new();
    let list: Vec<String> = e["aliases"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|a| a["alias"].as_str())
        .filter(|a| !a.eq_ignore_ascii_case(name))
        .filter(|a| seen.insert(a.to_lowercase()))
        .take(40)
        .map(|a| format!("<code>{}</code>", esc(a)))
        .collect();
    if !list.is_empty() {
        body.push_str(&format!(
            "<h2>Known as <span class=\"n\">{} name{}</span></h2><p class=\"aliases\">{}</p>",
            list.len(),
            if list.len() == 1 { "" } else { "s" },
            list.join("")
        ));
    }

    let offers: Vec<Value> = offerings
        .iter()
        .filter_map(|o| {
            let c = o["components"].as_array()?.iter().find(|c| {
                matches!(c["dimension"].as_str(), Some("mtok_in") | Some("image") | Some("call"))
            })?;
            Some(serde_json::json!({
                "@type": "Offer",
                "seller": {"@type": "Organization", "name": o["provider"]},
                "price": c["micros_per_unit"].as_i64().unwrap_or(0) as f64 / 1e6,
                "priceCurrency": "USD",
                "unitText": unit(c["dimension"].as_str().unwrap_or("")),
            }))
        })
        .collect();
    // SoftwareApplication, not Product: it is the schema.org type search
    // engines accept for software rich results, and everything here is
    // software sold as a service.
    let ld = serde_json::json!({
        "@context": "https://schema.org",
        "@type": "SoftwareApplication",
        "name": name,
        "description": lede,
        "brand": maker.map(|(_, n, _)| n.clone()),
        "applicationCategory": register,
        "operatingSystem": "Web",
        "offers": offers,
    })
    .to_string();
    // The visible opening sentence is the maker's, when it wrote one; the
    // search snippet is ours. 74 pages were sharing a description because
    // several products carry the same blurb from the same feed, and a
    // generated line is unique by construction.
    // A line for something with no price and no standing is short — "GitHub
    // Copilot — an agent from GitHub." — and a search engine wants a little
    // more than that. The first sentence of the paragraph carries it.
    let full = match about.paragraph.split_once(". ") {
        Some((first, _)) if about.line.len() < 90 => format!("{} {first}.", about.line),
        _ => about.line.clone(),
    };
    let snippet = match full.len() {
        0..=59 => lede.clone(),
        60..=300 => full,
        _ => {
            let cut = full[..300].rfind(", ").unwrap_or(300);
            format!("{}.", &full[..cut])
        }
    };
    Ok((snippet, body, ld))
}

// ---- routes ----------------------------------------------------------------

/// The browsing page. The first screen is rendered here so a phone on a slow
/// connection has a working list of links before any script runs; searching
/// and ordering arrive with a small index fetched afterwards. The page used to
/// carry the entire catalogue inside itself to do the same job.
async fn browse() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let find = ix.find_index()?;
        let empty = vec![];
        let things = find["things"].as_array().unwrap_or(&empty);
        let companies = find["companies"].as_array().unwrap_or(&empty);
        let mut first: Vec<&Value> = things.iter().collect();
        first.sort_by(|a, b| {
            b["s"].as_i64().unwrap_or(0).cmp(&a["s"].as_i64().unwrap_or(0)).then_with(|| {
                a["n"].as_str().unwrap_or("").cmp(b["n"].as_str().unwrap_or(""))
            })
        });
        let rows: String = first.iter().take(40).map(|x| row_html(x)).collect();
        let lede = format!(
            "Every AI model, tool and agent that is sold, who sells it and what it costs — \
             {} things from {} companies, read fresh every day.",
            things.len(),
            companies.len()
        );
        Ok(BROWSE
            .replace("__CSS__", STYLE)
            .replace("__CSS_V2__", CSS_V2)
            .replace("__THEME__", THEME_JS)
            .replace("__READ__", &esc(&ix.last_read()?))
            .replace("__LEDE__", &esc(&lede))
            .replace(
                "__COUNTS__",
                &format!("{} things · {} companies", things.len(), companies.len()),
            )
            .replace(
                "__HINT__",
                "The widest-sold first. Search and ordering wake up in a moment.",
            )
            .replace("__ANALYTICS__", ANALYTICS)
            .replace("__ROWS__", &rows))
    };
    match render() {
        Ok(page) => app(page),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("catalogue unavailable: {e}"),
        )
            .into_response(),
    }
}

/// The list's own index: enough to search and order 1,900 rows, and nothing
/// else. The whole catalogue lives at /index/pass_index_all.json, behind a
/// sign-in, named after what it is so it stays named that on the way out.
async fn find_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.find_index()) {
        Ok(v) => live_json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn all_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.export_json()) {
        Ok(v) => json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

fn find_entity(ix: &index::Index, head: &str, tail: &str) -> anyhow::Result<Option<Value>> {
    match ix.entity_at(head, tail)? {
        Some(id) => ix.entity_json(&id),
        None => Ok(None),
    }
}

/// Two constraints at once — /index/for/voice/licence/open — which is the
/// shape of the phrase people actually search and actually share.
async fn two(Path((a1, v1, a2, v2)): Path<(String, String, String, String)>) -> impl IntoResponse {
    let (v2, as_json) = match v2.strip_suffix(".json") {
        Some(v) => (v.to_string(), true),
        None => (v2, false),
    };
    let render = || -> anyhow::Result<Option<axum::response::Response>> {
        let ix = open()?;
        match ix.list_page(&[(a1.as_str(), v1.as_str()), (a2.as_str(), v2.as_str())])? {
            Some(page) => Ok(Some(if as_json {
                json(page)
            } else {
                html(list_html(&page, &rich_rows(&ix)?))
            })),
            None => Ok(None),
        }
    };
    match render() {
        Ok(Some(r)) => r,
        Ok(None) => missing(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn one(Path((head, tail)): Path<(String, String)>) -> impl IntoResponse {
    let (tail, as_json) = match tail.strip_suffix(".json") {
        Some(t) => (t.to_string(), true),
        None => (tail, false),
    };
    let render = || -> anyhow::Result<Option<axum::response::Response>> {
        let ix = open()?;
        // the reserved heads first: a company can never be called "board"
        if let Ok(n) = tail.parse::<usize>() {
            if let Some((page, data)) = hub(&ix, &head, n)? {
                return Ok(Some(if as_json { json(data) } else { html(page) }));
            }
        }
        if head == "top" {
            let Some(page) = ix.top_page(&tail)? else { return Ok(None) };
            return Ok(Some(if as_json {
                json(page)
            } else {
                html(top_html(&page, &ix.last_read()?))
            }));
        }
        // the axis lists: /index/for/voice, /index/licence/open, /index/does/…
        if ["for", "does", "register", "local"].contains(&head.as_str())
            || (head == "licence"
                && index::LICENCE_FAMILIES.iter().any(|(k, _, _)| *k == tail))
        {
            if let Some(page) = ix.list_page(&[(head.as_str(), tail.as_str())])? {
                return Ok(Some(if as_json {
                    json(page)
                } else {
                    html(list_html(&page, &rich_rows(&ix)?))
                }));
            }
            return Ok(None);
        }
        match head.as_str() {
            "board" => {
                let suite = ix
                    .all_suite_ids()?
                    .into_iter()
                    .find(|s| index::address_slug(s) == tail);
                let Some(suite) = suite else { return Ok(None) };
                let page = ix.board_page(&suite)?;
                return Ok(Some(if as_json { json(page) } else { html(board_html(&page)) }));
            }
            "task" => {
                // The same list used to live here. One address per list, or
                // two phrasings compete for the same reader.
                return Ok(Some(
                    (
                        StatusCode::MOVED_PERMANENTLY,
                        [(header::LOCATION, format!("/index/for/{tail}"))],
                    )
                        .into_response(),
                ));
            }
            "licence" => {
                let page = ix.facet_page(&head, &tail)?;
                if page["members"].as_array().map(|m| m.is_empty()).unwrap_or(true) {
                    return Ok(None);
                }
                return Ok(Some(if as_json { json(page) } else { html(facet_html(&head, &page)) }));
            }
            _ => {}
        }
        let Some(e) = find_entity(&ix, &head, &tail)? else {
            return Ok(None);
        };
        if as_json {
            return Ok(Some(json(e)));
        }
        let (lede, body, ld) = entity_body(&ix, &e)?;
        Ok(Some(html(shell(
            e["name"].as_str().unwrap_or("Product"),
            &lede,
            &format!("/index/{head}/{tail}"),
            body,
            ld,
        ))))
    };
    match render() {
        Ok(Some(r)) => r,
        Ok(None) => missing(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("catalogue unavailable: {e}"),
        )
            .into_response(),
    }
}

fn board_html(page: &Value) -> String {
    let name = page["name"].as_str().unwrap_or("Board");
    let measurer = page["measurer"].as_str().unwrap_or("");
    let empty = vec![];
    // On a board every row shares the same field, so a bar of "how many you
    // beat" is the rank column drawn twice. What the rank does not say is how
    // far apart the scores are — whether first is a nose ahead or a mile.
    let standings = page["standings"].as_array().unwrap_or(&empty);
    let lower = page["lower_is_better"].as_bool().unwrap_or(false);
    let vals: Vec<f64> = standings.iter().filter_map(|s| s["value"].as_f64()).collect();
    let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let rows: Vec<String> = standings
        .iter()
        .map(|s| {
            let place = match (s["rank"].as_i64(), s["out_of"].as_i64()) {
                (Some(r), Some(n)) => format!("{}<span class=\"of\">of {n}</span>", ordinal(r)),
                (Some(r), None) => ordinal(r),
                _ => "—".into(),
            };
            let field = match s["value"].as_f64() {
                Some(v) if hi > lo => {
                    let share = if lower { (hi - v) / (hi - lo) } else { (v - lo) / (hi - lo) };
                    let pct = (share * 100.0).clamp(2.0, 100.0);
                    format!("<span class=\"field\"><i style=\"width:{pct:.0}%\"></i></span>")
                }
                _ => String::new(),
            };
            format!(
                "<tr><td class=\"place{}\">{}</td><td><a href=\"{}\">{}</a>{field}</td>\
                 <td>{}</td><td class=\"score\">{}</td></tr>",
                if s["rank"].as_i64() == Some(1) { " top" } else { "" },
                place,          // built here from a number and a fixed span
                esc(s["href"].as_str().unwrap_or("/index")),
                esc(s["name"].as_str().unwrap_or(s["entity"].as_str().unwrap_or(""))),
                esc(s["metric"].as_str().unwrap_or("")),
                num(s["value"].as_f64().unwrap_or(0.0))
            )
        })
        .collect();
    let lede = if vals.is_empty() {
        format!("{name} is run by {measurer}.")
    } else {
        // "holds 171 of the models it has ranked" beside a column of rows
        // each saying "of 271" reads as two answers to one question. It is
        // two facts, and the sentence has to name both.
        let field = standings
            .iter()
            .filter_map(|r| r["out_of"].as_i64())
            .max()
            .unwrap_or(0);
        format!(
            "{name} is run by {measurer}. It has ranked {}, of which the catalogue holds {}, \
             scoring from {} to {} on {}.",
            if field > 0 { plural_n(field, "model") } else { "models".into() },
            grouped(rows.len() as i64),
            num(lo),
            num(hi),
            page["metric"].as_str().unwrap_or("its own metric")
        )
    };
    let read = standings
        .iter()
        .filter_map(|s| s["taken_at"].as_str())
        .max()
        .map(|d| format!("<p class=\"read\">Read from the board on {}</p>", esc(d)))
        .unwrap_or_default();
    let body = format!(
        "<h1>{}</h1><p class=\"lede\">{}</p><p class=\"io\">{}</p>\
         <div class=\"scroll\"><table class=\"grid rank\"><thead><tr><th>Place</th><th>Model</th>\
         <th>Metric</th><th>Score</th></tr></thead><tbody>{}</tbody></table></div>{read}",
        esc(name),
        esc(&lede),
        match page["url"].as_str() {
            Some(u) if !u.is_empty() => format!(
                "measured by {} · <a href=\"{}\" rel=\"nofollow noreferrer\">the board itself</a>",
                esc(measurer),
                safe_href(u)
            ),
            _ => format!("measured by {}", esc(measurer)),
        },
        rows.join("")
    );
    shell(name, &lede, page["href"].as_str().unwrap_or("/index"), body, String::new())
}

/// The rich row for every id, so a list can print prices without asking the
/// database again per member.
fn rich_rows(ix: &index::Index) -> anyhow::Result<HashMap<String, Value>> {
    let find = ix.find_index()?;
    let mut out = HashMap::new();
    for x in find["things"].as_array().into_iter().flatten() {
        if let Some(h) = x["h"].as_str() {
            out.insert(h.to_string(), x.clone());
        }
    }
    Ok(out)
}

/// The phrase a reader would type, built from the axes the page constrains:
/// "Open voice models", "Text to video models, tools and agents". The title is
/// the product here — a list nobody would search for is a list nobody wants.
fn list_phrase(axes: &[(String, String)], members: &[Value]) -> (String, String) {
    let mut adjective = String::new();
    let mut subject = String::new();
    let mut register = String::new();
    for (axis, value) in axes {
        match axis.as_str() {
            "licence" => {
                adjective = match value.as_str() {
                    "open" => "Open-weight ".into(),
                    "open-with-conditions" => "Conditionally licensed ".into(),
                    "noncommercial" => "Research-only ".into(),
                    _ => "Closed ".into(),
                }
            }
            "for" => subject = task_words(value),
            "does" => subject = value.replace(" → ", " to "),
            "local" => {
                adjective = format!(
                    "{} ",
                    value.replace("gb", " GB")
                );
                register = "models".into();
            }
            "register" => register = format!("{value}s"),
            _ => {}
        }
    }
    let kinds: std::collections::BTreeSet<&str> = members
        .iter()
        .filter_map(|m| m["register"].as_str())
        .collect();
    let what = if !register.is_empty() {
        register
    } else if kinds.len() == 1 {
        format!("{}s", kinds.iter().next().unwrap_or(&"thing"))
    } else {
        "models, tools and agents".to_string()
    };
    let local = axes.iter().find(|(a, _)| a == "local").map(|(_, v)| v.clone());
    let title = match &local {
        // "Models that run on 16 GB" says what the reader wants to know; "16 GB
        // models" says something that is not quite true of anything.
        Some(band) => {
            let gb = band.replace("gb", " GB");
            if subject.is_empty() {
                format!("Models that run on {gb}")
            } else {
                format!("{subject} models that run on {gb}")
            }
        }
        None if subject.is_empty() => format!("{adjective}{what}"),
        None => format!("{adjective}{subject} {what}"),
    };
    let title = title[..1].to_uppercase() + &title[1..];
    let n = members.len();
    let lede = match &local {
        Some(band) => {
            let gb: f64 = band.trim_end_matches("gb").parse().unwrap_or(16.0);
            let (_, _, device) = index::MEMORY_BANDS
                .iter()
                .find(|(k, _, _)| k == band)
                .unwrap_or(&("", 16.0, ""));
            format!(
                "{n} open-weight models whose weights fit in {} GB — {device}. Counted at \
                 four-bit quantisation, weights only, leaving the machine about a third of \
                 its memory and a gigabyte for context: room for roughly {:.0} billion \
                 parameters. A mixture of experts is counted in full, because it is held in \
                 full even though only a few experts compute.",
                band.trim_end_matches("gb"),
                index::fits_billions(gb)
            )
        }
        // The line has to name its own list, or two lists of the same size
        // hand a search engine the same sentence twice.
        None => format!(
            "{title}: {n} in the catalogue today{}. Every one with what it costs, \
             who sells it and where it stands.",
            if n < 10 { ", and the list grows as the market does" } else { "" }
        ),
    };
    (title, lede)
}

fn list_html(page: &Value, index: &HashMap<String, Value>) -> String {
    let empty = vec![];
    let members = page["members"].as_array().unwrap_or(&empty);
    let axes: Vec<(String, String)> = page["axes"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|a| Some((a["axis"].as_str()?.to_string(), a["value"].as_str()?.to_string())))
        .collect();
    let (title, mut lede) = list_phrase(&axes, members);
    if members.len() > PER_PAGE {
        lede.push_str(&format!(
            " The hundred shown here are the first of {}; the rest are a search away.",
            grouped(members.len() as i64)
        ));
    }
    // On a list about fitting in memory the load-bearing number is the size,
    // not what somebody else charges to host it. The price is still a click
    // away on the card.
    let local = axes.iter().any(|(a, _)| a == "local");
    // A list runs to a hundred like the hubs do. 435 rows was 126 kilobytes,
    // which is not a page, it is a download.
    let shown: Vec<&Value> = members.iter().take(PER_PAGE).collect();
    let rows: String = shown
        .iter()
        .map(|m| {
            if local {
                if let Some(p) = m["params"].as_i64() {
                    let b = p as f64 / 1e9;
                    let gb = b * 0.65;
                    let rich = m["href"].as_str().and_then(|h| index.get(h));
                    let sellers = rich
                        .and_then(|r| r["s"].as_i64())
                        .filter(|n| *n > 0)
                        .map(|n| format!("<span class=\"sc\">{n} also selling it hosted</span>"))
                        .unwrap_or_default();
                    return format!(
                        "<li><a href=\"{}\"><span class=\"nm\">{}</span>\
                         <span class=\"mk\">{}</span>\
                         <span class=\"pr\">{}B<span class=\"un\">≈{:.1} GB at 4-bit</span>\
                         </span>{sellers}</a></li>",
                        esc(m["href"].as_str().unwrap_or("/index")),
                        esc(m["name"].as_str().unwrap_or("")),
                        esc(rich.and_then(|r| r["m"].as_str()).unwrap_or("")),
                        // 14.77 rounded to 15 argues with a model called 14B
                        if b < 100.0 { format!("{b:.1}") } else { format!("{b:.0}") },
                        gb
                    );
                }
            }
            match m["href"].as_str().and_then(|h| index.get(h)) {
            Some(rich) => row_html(rich),
            None => format!(
                "<li><a href=\"{}\"><span class=\"nm\">{}</span>\
                 <span class=\"mk\">{}</span></a></li>",
                esc(m["href"].as_str().unwrap_or("/index")),
                esc(m["name"].as_str().unwrap_or("")),
                esc(m["register"].as_str().unwrap_or(""))
            ),
            }
        })
        .collect();
    // Where else to go from here: the same list without one of its
    // constraints, which is how a reader widens a search that came up short.
    let mut near = Vec::new();
    if axes.len() > 1 {
        for skip in 0..axes.len() {
            let kept: Vec<&(String, String)> =
                axes.iter().enumerate().filter(|(i, _)| *i != skip).map(|(_, a)| a).collect();
            let href = kept.iter().fold("/index".to_string(), |acc, (a, v)| {
                format!("{acc}/{a}/{}", index::address_slug(v))
            });
            let (t, _) = list_phrase(
                &kept.into_iter().cloned().collect::<Vec<_>>(),
                members,
            );
            near.push(format!("<a class=\"chip\" href=\"{href}\">{}</a>", esc(&t)));
        }
    }
    let wider = if near.is_empty() {
        String::new()
    } else {
        format!("<h2>Wider</h2><div class=\"chips\">{}</div>", near.join(""))
    };
    // What this list is, for whoever arrived from a search result. A paired
    // list gets both paragraphs: it is the intersection of two ideas and a
    // reader may know neither.
    let intro: String = axes
        .iter()
        .filter_map(|(axis, value)| {
            let key = if axis == "does" {
                value.replace(" → ", "-to-").replace(" + ", "-plus-").replace(' ', "-")
            } else {
                index::address_slug(value)
            };
            index::intro::intro(axis, &key)
        })
        .map(|p| format!("<p class=\"intro\">{}</p>", esc(p)))
        .collect();
    let body = format!(
        "<h1>{}</h1><p class=\"lede\">{}</p>{intro}<ul class=\"rows plain\">{rows}</ul>{wider}",
        esc(&title),
        esc(&under(&title, &lede))
    );
    // A list of nought, one or two is real and stays served, but it does not
    // go to the index: a search engine reading two thousand near-empty
    // facets marks the whole domain thin. It comes back when it fills.
    shell_with(
        &title,
        &lede,
        page["href"].as_str().unwrap_or("/index"),
        body,
        String::new(),
        members.len() < 3,
    )
}

fn facet_html(facet: &str, page: &Value) -> String {
    let value = page["name"].as_str().unwrap_or("");
    let empty = vec![];
    let members = page["members"].as_array().unwrap_or(&empty);
    // Count the registers so the line says what the reader is about to see,
    // rather than "39 things in the catalogue ocr", which is not a sentence.
    let mut by_reg = std::collections::BTreeMap::new();
    for m in members {
        *by_reg
            .entry(m["register"].as_str().unwrap_or("thing").to_string())
            .or_insert(0usize) += 1;
    }
    let breakdown = by_reg
        .iter()
        .map(|(r, n)| format!("{n} {r}{}", if *n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ");
    let lede = if facet == "task" {
        format!(
            "The catalogue holds {} things for {}: {breakdown}.",
            members.len(),
            task_words(value)
        )
    } else {
        format!(
            "The catalogue holds {} things published under {value}: {breakdown}.",
            members.len()
        )
    };
    let rows: Vec<String> = members
        .iter()
        .map(|m| {
            format!(
                "<li><a href=\"{}\">{}</a> <span class=\"reg\">{}</span></li>",
                esc(m["href"].as_str().unwrap_or("/index")),
                esc(m["name"].as_str().unwrap_or("")),
                esc(m["register"].as_str().unwrap_or(""))
            )
        })
        .collect();
    let cap = value
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string() + &value[c.len_utf8()..])
        .unwrap_or_else(|| value.to_string());
    let title = if facet == "task" {
        let words = task_words(value);
        let cap = words
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string() + &words[c.len_utf8()..])
            .unwrap_or(words.clone());
        format!("{cap} — models, tools and agents")
    } else {
        format!("{cap} — everything published under it")
    };
    let body = format!(
        "<h1>{}</h1><p class=\"lede\">{}</p><ul class=\"list\">{}</ul>",
        esc(&title),
        esc(&lede),
        rows.join("")
    );
    // A list of nought, one or two is real and stays served, but it does not
    // go to the index: a search engine reading two thousand near-empty
    // facets marks the whole domain thin. It comes back when it fills.
    shell_with(
        &title,
        &lede,
        page["href"].as_str().unwrap_or("/index"),
        body,
        String::new(),
        members.len() < 3,
    )
}

async fn company(Path(head): Path<String>) -> impl IntoResponse {
    let (head, as_json) = match head.strip_suffix(".json") {
        Some(h) => (h.to_string(), true),
        None => (head, false),
    };
    let render = || -> anyhow::Result<Option<axum::response::Response>> {
        let ix = open()?;
        if let Some(hub) = hub(&ix, &head, 1)? {
            return Ok(Some(if as_json { json(hub.1) } else { html(hub.0) }));
        }
        let found = ix
            .provider_addresses()?
            .into_iter()
            .find(|(_, _, s)| *s == head);
        let Some((id, _, _)) = found else {
            return Ok(None);
        };
        let page = ix.provider_page(&id)?;
        if as_json {
            return Ok(Some(json(page)));
        }
        Ok(Some(html(company_html(&page))))
    };
    match render() {
        Ok(Some(r)) => r,
        Ok(None) => missing(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("catalogue unavailable: {e}"),
        )
            .into_response(),
    }
}

fn company_html(page: &Value) -> String {
    let name = page["name"].as_str().unwrap_or("");
    let kind = page["provider_kind"].as_str().unwrap_or("vendor");
    let empty = vec![];
    let makes = page["makes"].as_array().unwrap_or(&empty);
    let resells = page["resells"].as_array().unwrap_or(&empty);
    // This line is the search snippet, so it should say what the company is
    // and what it costs to buy from, not just count rows.
    let registers = |rows: &Vec<Value>| -> String {
        let mut by = std::collections::BTreeMap::new();
        for r in rows {
            *by.entry(r["register"].as_str().unwrap_or("thing").to_string())
                .or_insert(0usize) += 1;
        }
        by.iter()
            .map(|(r, n)| format!("{n} {r}{}", if *n == 1 { "" } else { "s" }))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let lede = match (makes.len(), resells.len()) {
        (0, 0) => format!("{name} is in the catalogue as a {kind}, with nothing priced yet."),
        (_, 0) => format!(
            "{name} makes {} in the catalogue, each with what it costs and who else sells it.",
            registers(makes)
        ),
        (0, _) => format!(
            "{name} is an {kind}: it resells {} made by other companies, and the \
             catalogue holds what each one costs here.",
            registers(resells)
        ),
        (_, _) => format!(
            "{name} makes {} and also resells {}, with the price of each and who \
             else carries it.",
            registers(makes),
            registers(resells)
        ),
    };
    let list = |rows: &Vec<Value>| -> String {
        rows.iter()
            .map(|m| {
                format!(
                    "<li><a href=\"{}\">{}</a> <span class=\"reg\">{}</span></li>",
                    esc(m["href"].as_str().unwrap_or("/index")),
                    esc(m["name"].as_str().unwrap_or("")),
                    esc(m["register"].as_str().unwrap_or(""))
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    // The lede and the About's opening line were saying the same thing in two
    // sentences one after the other, which reads as a draft. The About line
    // is the better of the two — it is the one a reader can copy — so it is
    // the only one printed. The lede still goes to the search snippet.
    let mut body = format!(
        "<h1>{}</h1><p class=\"io\">{}{}</p>",
        esc(name),
        esc(kind),
        match page["url"].as_str() {
            Some(u) if !u.is_empty() => format!(
                " · <a href=\"{}\" rel=\"nofollow noreferrer\">{}</a>",
                safe_href(u),
                esc(u.trim_start_matches("https://").trim_start_matches("http://"))
            ),
            _ => String::new(),
        }
    );
    // What the encyclopedia says the company is, where we have read it. For a
    // company with nothing priced this is the whole page, and it is why the
    // page is worth having.
    if let Some(docs) = page["docs"].as_array() {
        if let Some(d) = docs.iter().find(|d| d["kind"] == "description") {
            body.push_str(&format!(
                "<p class=\"intro\">{}</p>",
                esc(d["text"].as_str().unwrap_or(""))
            ));
        }
    }
    // Venture money, which is the fact that makes a company with no products
    // worth listing at all.
    if let (Some(raised), Some(n)) = (page["raised"].as_i64(), page["rounds"].as_i64()) {
        let d = raised as f64;
        let said = if d >= 1e9 {
            format!("${:.1} billion", d / 1e9)
        } else {
            format!("${:.0} million", d / 1e6)
        };
        body.push_str(&format!(
            "<p class=\"note\">Has raised at least <em>{said}</em> across {} we could \
             read{}. A startup is a company running on somebody else's money, and this \
             is the evidence for it.</p>",
            plural_n(n, "round"),
            match page["raised_source"].as_str() {
                Some(u) if !u.is_empty() => format!(
                    " · <a href=\"{}\" rel=\"nofollow noopener\">source</a>", safe_href(u)),
                _ => String::new(),
            }
        ));
    }

    // The same three parts as a product's card, and in the same place: what
    // the company is, before the list of what it sells.
    body.push_str(&about_html(
        &index::about::provider(page),
        &open().and_then(|ix| ix.last_read()).unwrap_or_default(),
        page["href"].as_str().unwrap_or("/index"),
        name,
    ));

    // Money in and money out, each a plain list of names in alphabetical
    // order. A reader scanning for one name finds it faster in a sentence
    // than in a hundred-row table, and the relation is the whole content.
    let names = |rows: &Vec<Value>| -> String {
        rows.iter()
            .map(|x| {
                format!(
                    "<a href=\"{}\">{}</a>",
                    esc(x["href"].as_str().unwrap_or("/index")),
                    esc(x["name"].as_str().unwrap_or(""))
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(backers) = page["backers"].as_array() {
        if !backers.is_empty() {
            body.push_str(&format!(
                "<h2>Backed by <span class=\"n\">{}</span></h2><p class=\"roll\">{}</p>",
                backers.len(),
                names(backers)
            ));
        }
    }
    if let Some(portfolio) = page["portfolio"].as_array() {
        if !portfolio.is_empty() {
            body.push_str(&format!(
                "<h2>Has backed <span class=\"n\">{}</span></h2><p class=\"roll\">{}</p>",
                portfolio.len(),
                names(portfolio)
            ));
        }
    }

    if !makes.is_empty() {
        body.push_str(&format!(
            "<h2>Makes · {}</h2><ul class=\"list\">{}</ul>",
            makes.len(),
            list(makes)
        ));
    }
    if !resells.is_empty() {
        body.push_str(&format!(
            "<h2>Resells · {}</h2><ul class=\"list\">{}</ul>",
            resells.len(),
            list(resells)
        ));
    }
    let ld = serde_json::json!({
        "@context": "https://schema.org", "@type": "Organization",
        "name": name, "url": page["url"], "description": lede,
    })
    .to_string();
    // Ten products carry their company's name — Bolt, Vapi, Bland — and the
    // two pages would otherwise be titled the same thing.
    let title = format!("{name} — models, tools and prices");

    shell(&title, &lede, page["href"].as_str().unwrap_or("/index"), body, ld)
}

/// The four lists that are entry points rather than things. A hundred to a
/// page, in alphabetical order: 1,335 models on one page is a list nobody
/// reads and a payload nobody wants, and cutting it by letter would put a
/// reader who knows only half a name in the wrong place.
fn hub(ix: &index::Index, head: &str, page: usize) -> anyhow::Result<Option<(String, Value)>> {
    let (register, title) = match head {
        "models" => ("model", "Models"),
        "tools" => ("tool", "Tools"),
        "agents" => ("agent", "Agents"),
        "subscriptions" => ("subscription", "Subscriptions"),
        "providers" => ("provider", "Companies"),
        _ => return Ok(None),
    };
    let find = ix.find_index()?;
    let empty = vec![];
    let all: Vec<&Value> = if register == "provider" {
        find["companies"].as_array().unwrap_or(&empty).iter().collect()
    } else {
        find["things"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|x| x["r"] == register)
            .collect()
    };
    let pages = all.len().div_ceil(PER_PAGE).max(1);
    if page < 1 || page > pages {
        return Ok(None);
    }
    let slice = &all[(page - 1) * PER_PAGE..(page * PER_PAGE).min(all.len())];
    let first = slice.first().and_then(|x| x["n"].as_str()).unwrap_or("");
    let last = slice.last().and_then(|x| x["n"].as_str()).unwrap_or("");
    let lede = if pages == 1 {
        format!("Every {} in the catalogue — {} of them.", register, grouped(all.len() as i64))
    } else {
        format!(
            "Every {} in the catalogue — {} of them, a hundred to a page. \
             This page runs from {first} to {last}.",
            register,
            grouped(all.len() as i64)
        )
    };
    let items: String = slice.iter().map(|x| row_html(x)).collect();
    let at = |n: usize| {
        if n == 1 {
            format!("/index/{head}")
        } else {
            format!("/index/{head}/{n}")
        }
    };
    let nav = if pages > 1 {
        let numbers: String = (1..=pages)
            .map(|n| {
                if n == page {
                    format!("<span class=\"here\">{n}</span>")
                } else {
                    format!("<a href=\"{}\">{n}</a>", at(n))
                }
            })
            .collect();
        format!(
            "<nav class=\"pages\">{}{numbers}{}</nav>",
            if page > 1 {
                format!("<a class=\"step\" rel=\"prev\" href=\"{}\">Back</a>", at(page - 1))
            } else {
                String::new()
            },
            if page < pages {
                format!("<a class=\"step\" rel=\"next\" href=\"{}\">Next</a>", at(page + 1))
            } else {
                String::new()
            }
        )
    } else {
        String::new()
    };
    let heading = if pages > 1 {
        format!("{title} <span class=\"pg\">{page} of {pages}</span>")
    } else {
        title.to_string()
    };
    let body = format!(
        "<h1>{heading}</h1><p class=\"lede\">{}</p><ul class=\"rows plain\">{items}</ul>{nav}",
        esc(&lede)
    );
    // Fourteen pages of models all titled "Models" is fourteen pages competing
    // for one reader, and thirteen of them lose.
    let page_title = if pages > 1 {
        format!("{title}, page {page} of {pages}")
    } else {
        title.to_string()
    };
    Ok(Some((
        shell(&page_title, &lede, &at(page), body, String::new()),
        serde_json::json!({"kind": "hub", "name": head, "page": page, "pages": pages,
                           "members": slice}),
    )))
}

/// What is in here, said by the catalogue about itself — including what it
/// cannot say, which is the half a reference work usually leaves out.
/// The self-check, as marks. Green where the last run found nothing, amber
/// where it found something worth knowing, red where it found something that
/// would have stopped the deploy. A page that can only be green is not a
/// check, so the failing state is drawn as plainly as the passing one.
fn checks_html(v: &Value) -> String {
    let empty = Vec::new();
    let all = v["checks"].as_array().unwrap_or(&empty);
    if all.is_empty() {
        return String::new();
    }
    let suite = |name: &str, label: &str, blurb: &str| -> String {
        let rows: String = all
            .iter()
            .filter(|c| c["suite"].as_str() == Some(name))
            .map(|c| {
                let n = c["findings"].as_i64().unwrap_or(0);
                let blocking = c["blocking"].as_bool().unwrap_or(false);
                let (state, mark, said) = match (n, blocking) {
                    (0, _) => ("pass", "✓", "in place".to_string()),
                    (n, _) if n < 0 => ("fail", "!", "the check itself broke".to_string()),
                    (n, true) => ("fail", "✗", format!("{} to answer for", grouped(n))),
                    (n, false) => ("warn", "!", format!("{} worth knowing", grouped(n))),
                };
                format!(
                    "<li class=\"{state}\"><span class=\"mk\">{mark}</span>\
                     <span class=\"ck\"><b>{}</b><i>{}</i></span>\
                     <span class=\"vd\">{said}</span></li>",
                    esc(c["name"].as_str().unwrap_or("")),
                    esc(c["asks"].as_str().unwrap_or(""))
                )
            })
            .collect();
        if rows.is_empty() {
            return String::new();
        }
        format!(
            "<h3>{}</h3><p class=\"intro\">{}</p><ul class=\"checks\">{rows}</ul>",
            esc(label), esc(blurb)
        )
    };
    // Blocking and worth-knowing are different verdicts and the headline has
    // to say which it is: five amber notes read as a failed page otherwise,
    // and a page that cries wolf teaches a reader to stop looking.
    let stopped = all
        .iter()
        .filter(|c| {
            c["blocking"].as_bool().unwrap_or(false) && c["findings"].as_i64().unwrap_or(0) != 0
        })
        .count();
    let noted = v["failing"].as_i64().unwrap_or(0) as usize - stopped;
    let ran = v["ran_at"].as_str().unwrap_or("");
    let verdict = if stopped == 0 && noted == 0 {
        format!(
            "<p class=\"verdict all-clear\">All {} checks clear. Nothing to answer for.</p>",
            grouped(all.len() as i64)
        )
    } else if stopped == 0 {
        format!(
            "<p class=\"verdict all-clear\">Nothing blocking. {} of {} checks found nothing \
             at all; {} found something worth knowing, and none of it would make a page \
             tell a reader something untrue.</p>",
            grouped((all.len() - noted) as i64), grouped(all.len() as i64), grouped(noted as i64)
        )
    } else {
        format!(
            "<p class=\"verdict bad\">{} of {} checks found something that would stop a \
             deploy. The pages you are reading were built before it.</p>",
            grouped(stopped as i64), grouped(all.len() as i64)
        )
    };

    format!(
        "<section class=\"selfcheck\"><h2>What the catalogue checks about itself</h2>\
         <p class=\"intro\">These are not run when you open the page. They are the \
          recorded verdict of the last nightly walk — the same one that stops a \
          deploy when it fails, which is the only reason a mark here is worth \
          anything.</p>{verdict}{}{}\
         <p class=\"ranat\">Last consistency check: <b>{ran}</b>. It runs every night, \
          after the collector and before anybody reads the result.</p></section>",
        suite("database", "The catalogue itself",
              "Read against the database: duplication, contradiction, a row pointing at \
               nothing, a figure nobody has re-read in a while."),
        suite("pages", "What a reader is handed",
              "Read by walking every address in the sitemap and looking at what comes \
               back: the title, the date, the links, the JSON twin, the weight.")
    )
}

fn coverage_html(v: &Value, checks: &str) -> String {
    let n = |k: &str| v[k].as_i64().unwrap_or(0);
    let empty = vec![];
    let facts = [
        ("things", n("entities")),
        ("companies", n("providers")),
        ("ways to buy", n("ways")),
        ("price figures", n("figures")),
        ("standings", n("standings")),
        ("boards read", n("boards")),
        ("texts", n("texts")),
        ("names bound", n("aliases")),
    ]
    .iter()
    .map(|(label, num)| format!("<div><b>{}</b><span>{label}</span></div>", grouped(*num)))
    .collect::<String>();

    let breakdown = |key: &str, title: &str, href: Option<&str>| -> String {
        let rows = v[key]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .map(|r| {
                let k = r["k"].as_str().unwrap_or("");
                let label = match href {
                    Some(base) => format!(
                        "<a href=\"{base}/{}\">{}</a>",
                        index::address_slug(k),
                        esc(k)
                    ),
                    None => esc(k),
                };
                format!(
                    "<li><b>{}</b><span>{label}</span></li>",
                    grouped(r["n"].as_i64().unwrap_or(0))
                )
            })
            .collect::<String>();
        if rows.is_empty() {
            String::new()
        } else {
            format!("<h2>{}</h2><ul class=\"list gaps\">{rows}</ul>", esc(title))
        }
    };

    let gaps = v["gaps"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|g| {
            format!(
                "<li><b>{}</b><span>{}</span></li>",
                grouped(g["n"].as_i64().unwrap_or(0)),
                esc(g["what"].as_str().unwrap_or(""))
            )
        })
        .collect::<String>();

    let read = v["read_on"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|r| {
            format!(
                "<li><b>{}</b><span>figures read on {}</span></li>",
                grouped(r["n"].as_i64().unwrap_or(0)),
                esc(r["k"].as_str().unwrap_or(""))
            )
        })
        .collect::<String>();

    // A fifth of the figures come from two public price catalogues, not
    // from the sellers' own pages, and the page said otherwise. Say what is
    // true instead: where each figure came from is on the figure.
    let lede = format!(
        "The catalogue holds {} things sold by {} companies, {} ways to buy them and {} \
         price figures — most read from the seller's own page, the rest from public \
         price files, and every figure carries its source.",
        grouped(n("entities")),
        grouped(n("providers")),
        grouped(n("ways")),
        grouped(n("figures"))
    );
    let body = format!(
        "<h1>Status</h1><p class=\"lede\">{}</p>\
         <div class=\"facts\">{facts}</div>\
         <h2>What it cannot say</h2><ul class=\"list gaps\">{gaps}</ul>\
         <h2>When it was read</h2><ul class=\"list gaps\">{read}</ul>\
         {}{}{}{}{}{checks}\
         <h2>Waiting to be checked</h2><p class=\"intro\">\
          <a href=\"/index/quarantine\">The quarantine</a> holds what a feed offered and \
          the catalogue has not accepted — in its own database, so nothing in it is \
          counted, ranked or searched here. Work through it and what survives is promoted.</p>\
         <h2>The catalogue as data</h2><p class=\"intro\">\
          <a href=\"/index/pass_index_all.json\">Everything in one file</a> · \
          <a href=\"/index/coverage.json\">this page</a> · \
          <a href=\"/index/sitemap.xml\">every address</a>. Any page here answers \
          to its own address with <code>.json</code> on the end, and that twin is \
          open to anyone. The whole catalogue in one file asks you to \
          <a href=\"/signin?signin=/index/pass_index_all.json\">sign in</a> first, which is \
          free and takes an email address.</p>",
        esc(&lede),
        breakdown("by_register", "By register", None),
        breakdown("by_kind", "Companies by kind", None),
        breakdown("by_way", "Ways to buy", None),
        breakdown("by_task", "By what it does", Some("/index/for")),
        breakdown("by_licence", "By licence", Some("/index/licence")),
    );
    shell("Status", &lede, "/index/coverage", body, String::new())
}

/// What has not been checked yet.
///
/// Everything here arrived from a feed and stopped short of the catalogue:
/// nobody we have looked at sells it, or the company offering it is a name we
/// have never opened. It is kept because throwing it away would mean crawling
/// it again, and it is kept apart because a catalogue with one invented row
/// in it is worth less than a catalogue with a hundred missing ones.
async fn quarantine() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let con = rusqlite::Connection::open(pen())?;
        let mut q = con.prepare(
            "SELECT id, kind, name, COALESCE(maker,''), why, held_since, sellers, \
                    COALESCE(low,0), COALESCE(dimension,'') \
               FROM candidates ORDER BY kind, sellers DESC, name",
        )?;
        let rows: Vec<(String, String, String, String, String, String, i64, i64, String)> = q
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                    r.get(6)?, r.get(7)?, r.get(8)?))
            })?
            .collect::<std::result::Result<_, _>>()?;

        let mut why: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        for r in &rows {
            *why.entry(r.4.as_str()).or_default() += 1;
            *kinds.entry(r.1.clone()).or_default() += 1;
        }
        let facts: String = kinds
            .iter()
            .map(|(k, n)| format!("<div><b>{}</b><span>{}</span></div>", grouped(*n as i64),
                                  esc(&plural(*n, k))))
            .collect();
        let held: String = rows
            .iter()
            .map(|(id, kind, name, maker, why, since, sellers, low, dim)| {
                let rate = if *low > 0 {
                    format!("<span class=\"pr\">{}<span class=\"un\">{}</span></span>",
                            money(*low), esc(&unit(dim)))
                } else {
                    String::new()
                };
                format!(
                    "<li data-k=\"{}\"><a href=\"#{}\"><span class=\"nm\">{}</span>\
                     <span class=\"mk\">{}{}{}</span>{rate}\
                     <span class=\"sc\">{}</span></a></li>",
                    esc(kind),
                    esc(id),
                    esc(name),
                    if maker.is_empty() { String::new() } else { format!("{} · ", esc(maker)) },
                    esc(why),
                    if *sellers > 0 {
                        format!(" · {}", plural(*sellers as usize, "offer"))
                    } else {
                        String::new()
                    },
                    esc(since)
                )
            })
            .collect();
        let reasons: String = why
            .iter()
            .map(|(w, n)| format!("<li><b>{}</b><span>{}</span></li>", grouped(*n as i64), esc(w)))
            .collect();

        let body = format!(
            "<h1>Quarantine</h1><p class=\"lede\">Everything a feed offered that the \
             catalogue has not accepted. It is held in its own database, so nothing here \
             is counted, ranked, searched or linked anywhere in the index — a missing \
             entry costs a reader one lookup, an invented one costs them the whole \
             catalogue. Work through it and what survives is promoted.</p>\
             <div class=\"facts\">{facts}</div>\
             <h2>Why they are here</h2><ul class=\"list gaps\">{reasons}</ul>\
             <h2>Held <span class=\"n\">{}</span></h2>\
             <input id=\"pq\" type=\"search\" placeholder=\"Search what is held…\" \
              autocomplete=\"off\">\
             <ul class=\"rows plain\" id=\"prows\">{held}</ul>\
             <script>(function(){{var q=document.getElementById(\"pq\"),\
              l=document.getElementById(\"prows\");if(!q||!l)return;\
              q.addEventListener(\"input\",function(){{var v=q.value.trim().toLowerCase();\
              [].forEach.call(l.children,function(li){{\
                li.style.display=!v||li.textContent.toLowerCase().indexOf(v)>-1?\"\":\"none\"}})}})}})();\
             </script>",
            grouped(rows.len() as i64)
        );
        Ok(shell_with(
            "Quarantine",
            "What the catalogue has not accepted yet, held apart from it.",
            "/index/quarantine",
            body,
            String::new(),
            true,
        ))
    };
    match render() {
        Ok(page) => app(page),
        Err(e) => app(format!("<h1>Quarantine</h1><p class=\"none\">{}</p>", esc(&e.to_string()))),
    }
}

/// "1 model", "296 models", "72 companies" — a count and its noun agreeing.
fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        return one.to_string();
    }
    match one {
        "company" => "companies".into(),
        w if w.ends_with('y') => format!("{}ies", &w[..w.len() - 1]),
        w if w.ends_with('s') || w.ends_with("ch") || w.ends_with("sh") => format!("{w}es"),
        w => format!("{w}s"),
    }
}

async fn tech_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.terms()) {
        Ok(t) => live_json(serde_json::json!({"href": "/index/tech", "kind": "vocabulary",
                                              "count": t.len(), "terms": t})),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}

/// How big a model has to be to lead its category for under a dollar.
///
/// A buyer's question with a number for an answer: if I will not pay more
/// than a dollar the million tokens, what size of model am I looking at?
/// Asked of the four categories the Top page is built on, and answered from
/// the three best-placed models each one can be bought at that price.
///
/// Most of the leaders at this price are closed and publish no size. Dropping
/// them would answer a different question — how big the best *open* cheap
/// models are — so instead each unpublished size is estimated as the median
/// of the three models nearest it in standing that do publish one, and every
/// estimate says on the page that it is one.
async fn bang() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let fams = ix.dollar_by_family(1_000_000)?;
        let measured: i64 = fams.iter().filter_map(|f| f["measured"].as_i64()).sum();

        let cards: String = fams
            .iter()
            .map(|f| {
                // The Overall card carries the two figures for the whole
                // page and lists nothing.
                if let Some(avg_out) = f["average_out"].as_i64() {
                    return format!(
                        "<div class=\"card\"><h3>Overall</h3>\
                         <p class=\"big\">{}</p>\
                         <p class=\"cap\">average size across the four tops</p>\
                         <p class=\"big\">{}</p>\
                         <p class=\"cap\">average price, the million tokens out</p>\
                         <p class=\"io\">{} models picked across the four categories</p></div>",
                        billions(f["average_params"].as_i64().unwrap_or(0)),
                        money(avg_out),
                        grouped(f["models"].as_i64().unwrap_or(0)),
                    );
                }
                let picks: String = f["top"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|m| {
                                let est = m["estimated"].as_bool().unwrap_or(false);
                                // A pick whose size nothing could supply shows
                                // a dash, not a fabricated "0B".
                                let size = match m["params"].as_i64() {
                                    Some(p) => format!(
                                        "<span class=\"sz{}\">{}{}</span>",
                                        if est { " est" } else { "" },
                                        if est { "about " } else { "" },
                                        billions(p),
                                    ),
                                    None => "<span class=\"sz\">size not published</span>".to_string(),
                                };
                                format!(
                                    "<li><a class=\"nm\" href=\"{}\">{}</a>{}\
                                     <span class=\"px\">{}</span></li>",
                                    esc(m["href"].as_str().unwrap_or("#")),
                                    esc(m["name"].as_str().unwrap_or("")),
                                    size,
                                    money(m["out"].as_i64().unwrap_or(0)),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                format!(
                    "<div class=\"card\"><h3><a href=\"{list}\">{}</a></h3>\
                     <p class=\"big\">{}</p>\
                     <p class=\"cap\">average size of the top three by benchmarks</p>\
                     <ol class=\"picks\">{picks}</ol>\
                     <p class=\"io\"><a href=\"{list}\">all {} measured here under a dollar</a></p></div>",
                    esc(f["family"].as_str().unwrap_or("")),
                    billions(f["average_params"].as_i64().unwrap_or(0)),
                    grouped(f["measured"].as_i64().unwrap_or(0)),
                    list = esc(f["list"].as_str().unwrap_or("#")),
                )
            })
            .collect();

        let body = format!(
            "<h1>Bang for the buck</h1>\
             <p class=\"lede\">The three best models in each of the four categories — \
             best by that category's own leaderboards, benchmarks first — among those \
             costing under a dollar the million tokens. And the average of their sizes: \
             how big a model leads its category at that price.</p>\
             <div class=\"facts\">\
               <div><b>{avg_b}</b><span>billion parameters, the average of the four tops</span></div>\
               <div><b>{avg_px}</b><span>the average price, a million tokens out</span></div>\
               <div><b>{}</b><span>models measured and under a dollar</span></div>\
             </div>\
             <div class=\"cards\">{cards}</div>\
             <h2>How this is worked out</h2>\
             <p class=\"intro\">A model counts if a company sells its output for under a \
             dollar the million tokens on the ordinary lane — the current rate, not the \
             cheapest ever recorded — and if it places on one of that category's own boards. \
             The three shown are the top three by benchmarks: sorted by the model's best \
             placing among that category's boards, as a share of the field, price breaking \
             ties.</p>\
             <p class=\"intro\">Most leaders at this price are closed and publish no \
             parameter count. Leaving them out would answer a different question, so each is \
             shown as <i>about</i> the median size of the three models nearest it in standing \
             that do publish one. Every such figure is marked; the rest are the makers' own.</p>\
             <p class=\"io\"><a href=\"/index/1dollar\">every model under a dollar</a> · \
             <a href=\"/index/top\">the categories</a> · \
             <a href=\"/index/sizes\">models by size</a> · \
             <a href=\"/index/bang.json\">this page as JSON</a></p>",
            grouped(measured),
            avg_b = fams.first()
                .and_then(|f| f["average_out"].as_i64().and(f["average_params"].as_i64()))
                .map(billions).unwrap_or_default(),
            avg_px = fams.first()
                .and_then(|f| f["average_out"].as_i64())
                .map(money).unwrap_or_default(),
        );
        Ok(shell(
            "Bang for the buck — how big a model leads its category under $1",
            "The best models under a dollar the million tokens, by category, and how big they are.",
            "/index/bang",
            body,
            String::new(),
        ))
    };
    match render() {
        Ok(page) => html(page),
        Err(e) => html(format!("<h1>Bang for the buck</h1><p class=\"none\">{}</p>",
                               esc(&e.to_string()))),
    }
}

async fn bang_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.dollar_by_family(1_000_000)) {
        Ok(fams) => live_json(serde_json::json!({
            "href": "/index/bang",
            "kind": "bang-for-the-buck",
            "rule": "within each of the four categories, the three best-placed models whose \
                     output a company sells for under $1 per million tokens on the ordinary \
                     lane, and the average of their parameter counts. A size the maker does \
                     not publish is estimated as the median of the three models nearest it in \
                     standing that do, and is marked estimated.",
            "ceiling_micros_per_mtok_out": 1_000_000,
            "categories": fams,
        })),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}

async fn dollar_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.dollar_models(1_000_000)) {
        Ok(rows) => live_json(serde_json::json!({
            "href": "/index/1dollar",
            "kind": "dollar-models",
            "rule": "places on a board, and one company sells its output for under \
                     $1 per million tokens on the ordinary lane",
            "count": rows.len(),
            "models": rows,
        })),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}

/// Models that place on a board and cost under a dollar to run.
///
/// A cheap model nobody has measured is a cheap unknown, and a low price on
/// a free tier is an allowance somebody can withdraw. So both conditions are
/// hard: it stands somewhere, and a company sells its output for under a
/// dollar the million tokens on the ordinary lane, at a price you can pay.
async fn dollar() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let rows = ix.dollar_models(1_000_000)?;
        let makers: std::collections::BTreeSet<&str> =
            rows.iter().filter_map(|r| r["maker"].as_str()).filter(|m| !m.is_empty()).collect();
        let cheapest = rows.iter().filter_map(|r| r["out"].as_i64()).min().unwrap_or(0);

        let body_rows: String = rows
            .iter()
            .map(|r| {
                let out = r["out"].as_i64().unwrap_or(0);
                let inn = r["in"].as_i64();
                let place = match (r["rank"].as_i64(), r["field"].as_i64()) {
                    (Some(rk), Some(f)) => format!(
                        "<span class=\"place\">{}<span class=\"of\">of {}</span></span>",
                        ordinal(rk), grouped(f)),
                    _ => String::new(),
                };
                format!(
                    "<tr data-n=\"{}\" data-p=\"{out}\" data-s=\"{}\" data-b=\"{}\">\
                     <td><a href=\"{}\">{}</a></td>\
                     <td>{}</td>\
                     <td>{place}<i>{}</i></td>\
                     <td class=\"figs\">{}<span class=\"fig\"><b>{}</b>\
                       <span class=\"u\">out</span></span></td>\
                     <td class=\"score\">{}</td></tr>",
                    esc(&r["name"].as_str().unwrap_or("").to_lowercase()),
                    r["sellers"].as_i64().unwrap_or(0),
                    r["boards"].as_i64().unwrap_or(0),
                    esc(r["href"].as_str().unwrap_or("#")),
                    esc(r["name"].as_str().unwrap_or("")),
                    esc(r["maker"].as_str().unwrap_or("")),
                    esc(r["board"].as_str().unwrap_or("")),
                    inn.map(|v| format!(
                        "<span class=\"fig\"><b>{}</b><span class=\"u\">in</span></span>",
                        money(v)))
                        .unwrap_or_default(),
                    money(out),
                    grouped(r["sellers"].as_i64().unwrap_or(0)),
                )
            })
            .collect();

        let body = format!(
            "<h1>$1 models</h1><p class=\"lede\">Models that place on a leaderboard and \
             whose output somebody sells for under a dollar the million tokens. Both halves \
             are the point: a cheap model nobody has measured is a cheap unknown, and a \
             price that only holds on a free tier is an allowance rather than a price. \
             The rate shown is the lowest a company charges on its ordinary lane.</p>\
             <div class=\"facts\">\
               <div><b>{}</b><span>models</span></div>\
               <div><b>{}</b><span>makers</span></div>\
               <div><b>{}</b><span>the cheapest output</span></div>\
             </div>\
             <h2>All of them <span class=\"n\">{}</span></h2><div class=\"scroll\">\
             <table class=\"grid sortable\"><thead><tr>\
               <th><button type=\"button\" data-by=\"n\">Model</button></th>\
               <th>Maker</th><th>Best place</th>\
               <th><button type=\"button\" data-by=\"p\" class=\"on\">Rate</button></th>\
               <th><button type=\"button\" data-by=\"s\">Sellers</button></th>\
             </tr></thead><tbody>{body_rows}</tbody></table></div>",
            grouped(rows.len() as i64),
            grouped(makers.len() as i64),
            money(cheapest),
            grouped(rows.len() as i64),
        );
        Ok(shell(
            "$1 models — good, measured, and under a dollar",
            "Models that place on a leaderboard and cost under a dollar the million tokens.",
            "/index/1dollar",
            body,
            String::new(),
        ))
    };
    match render() {
        Ok(page) => html(page),
        Err(e) => html(format!("<h1>$1 models</h1><p class=\"none\">{}</p>",
                               esc(&e.to_string()))),
    }
}

/// The vocabulary, A to Z.
async fn tech() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let terms = ix.terms()?;
        let mut letters: std::collections::BTreeMap<char, Vec<&Value>> = Default::default();
        for t in &terms {
            let c = t["term"].as_str().unwrap_or("?").chars().next().unwrap_or('?')
                .to_ascii_uppercase();
            letters.entry(c).or_default().push(t);
        }
        let jump: String = letters
            .keys()
            .map(|c| format!("<a href=\"#{c}\">{c}</a>"))
            .collect();
        let blocks: String = letters
            .iter()
            .map(|(c, ts)| {
                let rows: String = ts
                    .iter()
                    .map(|t| {
                        format!(
                            "<li><a href=\"{}\"><span class=\"nm\">{}</span>\
                             <span class=\"mk\">{}</span></a></li>",
                            esc(t["href"].as_str().unwrap_or("")),
                            esc(t["term"].as_str().unwrap_or("")),
                            esc(t["short"].as_str().unwrap_or("")),
                        )
                    })
                    .collect();
                format!("<h2 id=\"{c}\">{c}</h2><ul class=\"rows plain terms\">{rows}</ul>")
            })
            .collect();
        let body = format!(
            "<h1>The vocabulary</h1><p class=\"lede\">What the words on the rest of this \
             site mean — what you are billed for, how the things work, and how they are \
             sold. Each entry answers in its first sentence.</p>\
             <nav class=\"jump atoz\">{jump}</nav>{blocks}",
        );
        Ok(shell("The vocabulary", "What the words mean: tokens, context, MoE, MCP, x402.",
                 "/index/tech", body, String::new()))
    };
    match render() {
        Ok(page) => html(page),
        Err(e) => html(format!("<h1>The vocabulary</h1><p class=\"none\">{}</p>",
                               esc(&e.to_string()))),
    }
}

/// One word, answered.
async fn tech_one(Path(slug): Path<String>) -> impl IntoResponse {
    if let Some(bare) = slug.strip_suffix(".json") {
        return match open().and_then(|ix| ix.term(bare)) {
            Ok(Some(t)) => live_json(t),
            _ => live_json(serde_json::json!({"error": "not a word we hold", "slug": bare})),
        };
    }
    let render = || -> anyhow::Result<Option<String>> {
        let ix = open()?;
        let Some(t) = ix.term(&slug)? else { return Ok(None) };
        let also: Vec<String> = t["also"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| format!(
                "<span class=\"chip flat\">{}</span>", esc(x))).collect())
            .unwrap_or_default();
        let see: Vec<String> = t["see"]
            .as_array()
            .map(|a| a.iter().map(|x| format!(
                "<li><a href=\"{}\"><span class=\"nm\">{}</span></a></li>",
                esc(x["href"].as_str().unwrap_or("")),
                esc(x["label"].as_str().unwrap_or("")))).collect())
            .unwrap_or_default();
        let others: String = ix
            .terms()?
            .iter()
            .filter(|o| o["kind"] == t["kind"] && o["slug"] != t["slug"])
            .take(8)
            .map(|o| format!("<a class=\"chip\" href=\"{}\">{}</a>",
                             esc(o["href"].as_str().unwrap_or("")),
                             esc(o["term"].as_str().unwrap_or(""))))
            .collect();
        let body = format!(
            "<h1>{}</h1><p class=\"lede\">{}</p>\
             <p class=\"io\">{} · <a href=\"/index/tech\">the vocabulary</a></p>\
             {}\
             <p class=\"intro\">{}</p>\
             {}\
             <h2>Nearby</h2><div class=\"chips\">{others}</div>",
            esc(t["term"].as_str().unwrap_or("")),
            esc(t["short"].as_str().unwrap_or("")),
            esc(t["kind"].as_str().unwrap_or("")),
            if also.is_empty() {
                String::new()
            } else {
                format!("<div class=\"chips\">{}</div>", also.join(""))
            },
            esc(t["body"].as_str().unwrap_or("")),
            if see.is_empty() {
                String::new()
            } else {
                format!("<h2>In the catalogue</h2><ul class=\"rows plain\">{}</ul>",
                        see.join(""))
            },
        );
        // A word and a product can share a name — Recraft sells Inpainting and
        // the vocabulary explains it. The title says which page this is.
        let short = t["short"].as_str().unwrap_or("");
        let body_text = t["body"].as_str().unwrap_or("");
        let blurb = if short.len() >= 110 {
            short.to_string()
        } else {
            let first = body_text.split_terminator(". ").next().unwrap_or("");
            format!("{short} {first}.").trim().to_string()
        };
        Ok(Some(shell(
            &format!("{} — what it means", t["term"].as_str().unwrap_or("")),
            &blurb,
            &format!("/index/tech/{slug}"),
            body,
            String::new(),
        )))
    };
    match render() {
        Ok(Some(page)) => html(page),
        _ => (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
              "<h1>Not a word we hold</h1><p><a href=\"/index/tech\">The vocabulary</a></p>"
                  .to_string())
            .into_response(),
    }
}

/// Companies running on venture money.
async fn startups() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let rows = ix.startups()?;
        let total: i64 = rows.iter().filter_map(|r| r["raised"].as_i64()).sum();
        let selling = rows.iter().filter(|r| r["sells"].as_i64().unwrap_or(0) > 0).count();
        let priced = rows.iter().filter(|r| r["raised"].as_i64().unwrap_or(0) > 0).count();

        // Dollars, said the way money is said: billions to one decimal.
        let sum = |v: i64| -> String {
            let d = v as f64;
            if d >= 1e12 {
                format!("${:.1}T", d / 1e12)
            } else if d >= 1e9 {
                format!("${:.1}B", d / 1e9)
            } else if d >= 1e6 {
                format!("${:.0}M", d / 1e6)
            } else {
                format!("${}", grouped(v))
            }
        };

        let body_rows: String = rows
            .iter()
            .map(|r| {
                let raised = r["raised"].as_i64();
                let n = r["rounds"].as_i64().unwrap_or(0);
                let sells = r["sells"].as_i64().unwrap_or(0);
                let makes = r["makes"].as_i64().unwrap_or(0);
                let does = match (makes, sells) {
                    (0, 0) => r["what"]
                        .as_str()
                        .map(|w| w.chars().take(96).collect::<String>())
                        .unwrap_or_else(|| "nothing of theirs is priced here".into()),
                    (m, 0) => format!("{} in the catalogue", plural_n(m, "model")),
                    (0, sd) => format!("resells {}", plural_n(sd, "thing")),
                    (m, sd) => format!("{}, and sells {}", plural_n(m, "model"),
                                       plural_n(sd, "thing")),
                };
                // A figure where we could read one; otherwise the backer, which
                // is the evidence that there was money at all.
                let (figure, note) = match raised {
                    Some(v) if v > 0 => (
                        format!("<span class=\"fig\"><b>{}</b><span class=\"u\">across {}</span>\
                                 </span>", sum(v), plural_n(n, "round")),
                        String::new(),
                    ),
                    _ => (
                        format!("<span class=\"fig\"><span class=\"u\">{}</span></span>",
                                esc(r["backing"].as_str().unwrap_or("backed"))),
                        String::new(),
                    ),
                };
                let src = r["source"].as_str().unwrap_or("");
                format!(
                    "<tr data-n=\"{}\" data-p=\"{}\" data-s=\"{sells}\">\
                     <td><a href=\"{}\">{}</a>{note}</td>\
                     <td>{}</td>\
                     <td class=\"figs\">{figure}</td>\
                     <td class=\"score\">{}</td></tr>",
                    esc(&r["name"].as_str().unwrap_or("").to_lowercase()),
                    raised.unwrap_or(0),
                    esc(r["href"].as_str().unwrap_or("#")),
                    esc(r["name"].as_str().unwrap_or("")),
                    esc(&does),
                    if src.is_empty() {
                        String::new()
                    } else {
                        format!("<a href=\"{}\" rel=\"nofollow noopener\">read</a>", esc(src))
                    },
                )
            })
            .collect();

        let body = format!(
            "<h1>Startups</h1><p class=\"lede\">Companies here that run on venture capital. \
             A startup is not a size or an age — it is a company spending somebody else's \
             money — so the mark is earned by a round we could read, and a company without \
             one is missing from this list rather than absent from the market.</p>\
             <div class=\"facts\">\
               <div><b>{}</b><span>companies</span></div>\
               <div><b>{}</b><span>raised by the {} we have a figure for</span></div>\
               <div><b>{}</b><span>of them sell something here</span></div>\
             </div>\
             <p class=\"note\">A company earns its place two ways. Y Combinator publishes \
              its portfolio and invests in every company it admits, so the batch is the \
              evidence and the amount is unknown. Otherwise the figure is the rounds stated \
              in the company's own Wikipedia article, added up — a valuation is not counted, \
              because it is what somebody thinks a company is worth rather than money it \
              received. Where neither source says anything the company is simply not here: \
              <em>we did not read it</em> is not the same sentence as <em>it did not \
              happen</em>.</p>\
             <h2>All of them <span class=\"n\">{}</span></h2><div class=\"scroll\">\
             <table class=\"grid sortable\"><thead><tr>\
               <th><button type=\"button\" data-by=\"n\">Company</button></th>\
               <th>What they do here</th>\
               <th><button type=\"button\" data-by=\"p\" class=\"on\">Raised</button></th>\
               <th>Source</th>\
             </tr></thead><tbody>{body_rows}</tbody></table></div>",
            grouped(rows.len() as i64),
            sum(total),
            grouped(priced as i64),
            grouped(selling as i64),
            grouped(rows.len() as i64),
        );
        Ok(shell("Startups — the companies running on venture money",
                 "Companies in the catalogue that have raised venture capital, and how much.",
                 "/index/startups", body, String::new()))
    };
    match render() {
        Ok(page) => html(page),
        Err(e) => html(format!("<h1>Startups</h1><p class=\"none\">{}</p>",
                               esc(&e.to_string()))),
    }
}

async fn startups_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.startups()) {
        Ok(rows) => live_json(serde_json::json!({
            "href": "/index/startups", "kind": "startups",
            "rule": "a venture round stated in the company's own Wikipedia article; \
                     valuations are not counted",
            "count": rows.len(), "companies": rows})),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}

/// "1 round", "4 rounds" — with the count in front.
fn plural_n(n: i64, one: &str) -> String {
    format!("{} {}", grouped(n), plural(n.max(0) as usize, one))
}

/// Every model whose size somebody published, filtered by band and sorted by
/// whatever the reader is actually choosing on.
///
/// Size is the filter, not the order: a reader comes here knowing what will
/// fit on their machine and wants the best of what fits. So the bands are
/// tabs, and the order starts at how many companies sell a model — the
/// closest thing the catalogue has to a vote.
async fn sizes() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let rows = ix.sized_models()?;
        let mut per_band: std::collections::BTreeMap<&str, usize> = Default::default();
        for r in &rows {
            *per_band.entry(r["band"].as_str().unwrap_or("")).or_default() += 1;
        }
        let tabs: String = std::iter::once(format!(
            "<button class=\"on\" data-band=\"all\">All <i>{}</i></button>",
            grouped(rows.len() as i64)
        ))
        .chain(index::SIZE_BANDS.iter().map(|(key, label, lo, hi, _)| {
            let n = per_band.get(key).copied().unwrap_or(0);
            let range = if hi.is_infinite() {
                format!("{:.0}T+", lo / 1000.0)
            } else if *lo == 0.0 {
                format!("under {hi:.0}B")
            } else if *hi >= 1000.0 {
                format!("{lo:.0}B–1T")
            } else {
                format!("{lo:.0}–{hi:.0}B")
            };
            format!(
                "<button data-band=\"{key}\"{}>{label} <i>{range} · {}</i></button>",
                if n == 0 { " disabled" } else { "" },
                grouped(n as i64)
            )
        }))
        .collect();

        let body_rows: String = rows
            .iter()
            .map(|r| {
                let b = r["billions"].as_f64().unwrap_or(0.0);
                let size = if b >= 1000.0 {
                    format!("{:.1}T", b / 1000.0)
                } else if b >= 10.0 {
                    format!("{b:.0}B")
                } else {
                    format!("{b:.1}B")
                };
                let price = match (r["in"].as_i64(), r["out"].as_i64()) {
                    (Some(i), Some(o)) => format!(
                        "<span class=\"pr\">{}<span class=\"to\">→</span>{}\
                         <span class=\"un\">per Mtok in / out</span></span>",
                        money(i), money(o)),
                    (Some(i), None) => format!(
                        "<span class=\"pr\">{}<span class=\"un\">per Mtok in</span></span>",
                        money(i)),
                    _ => String::new(),
                };
                let boards = r["boards"].as_i64().unwrap_or(0);
                format!(
                    "<li data-band=\"{}\" data-p=\"{}\" data-s=\"{}\" data-b=\"{}\" \
                        data-n=\"{}\" data-z=\"{}\">\
                     <a href=\"{}\"><span class=\"nm\">{}</span>\
                     <span class=\"mk\">{}<b>{size}</b> · about {} GB to hold{}</span>\
                     {price}<span class=\"sc\">{}</span></a></li>",
                    esc(r["band"].as_str().unwrap_or("")),
                    r["in"].as_i64().unwrap_or(i64::MAX / 2),
                    r["sellers"].as_i64().unwrap_or(0),
                    boards,
                    esc(&r["name"].as_str().unwrap_or("").to_lowercase()),
                    r["billions"].as_f64().unwrap_or(0.0),
                    esc(r["href"].as_str().unwrap_or("#")),
                    esc(r["name"].as_str().unwrap_or("")),
                    match r["maker"].as_str().unwrap_or("") {
                        "" => String::new(),
                        m => format!("{} · ", esc(m)),
                    },
                    match r["gb"].as_f64().unwrap_or(0.0) {
                        g if g >= 100.0 => format!("{g:.0}"),
                        g => format!("{g:.1}"),
                    },
                    if boards > 0 {
                        format!(" · {}", plural_n(boards, "board"))
                    } else {
                        String::new()
                    },
                    match r["sellers"].as_i64().unwrap_or(0) {
                        0 => "nobody sells it".to_string(),
                        n => format!("{n} selling"),
                    },
                )
            })
            .collect();

        let body = format!(
            "<h1>Models by size</h1><p class=\"lede\">The {} models whose parameter count \
             somebody published, and what each one takes to hold: about 0.65 GB per billion \
             at four-bit quantisation, plus a gigabyte for the context and the runtime. \
             A model whose maker publishes no count is not here — guessing it from a price \
             would invent the one number this page exists to report.</p>\
             <nav class=\"bands\" id=\"bands\">{tabs}</nav>\
             <div class=\"top-row\"><div class=\"sorts\" id=\"ssort\">\
               <button class=\"on\" data-by=\"s\">Sellers</button>\
               <button data-by=\"b\">Boards</button>\
               <button data-by=\"p\">Price</button>\
               <button data-by=\"z\">Size</button>\
               <button data-by=\"n\">Name</button>\
             </div><span class=\"counts\" id=\"scount\"></span></div>\
             <ul class=\"rows plain\" id=\"srows\">{body_rows}</ul>\
             <script>{}</script>",
            grouped(rows.len() as i64),
            SIZES_JS
        );
        Ok(shell("Models by size — what fits on what",
                 "Every model whose parameter count is published, by size band, with what \
                  it takes to hold and who sells it.",
                 "/index/sizes", body, String::new()))
    };
    match render() {
        Ok(page) => html(page),
        Err(e) => html(format!("<h1>Models by size</h1><p class=\"none\">{}</p>",
                               esc(&e.to_string()))),
    }
}

async fn sizes_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.sized_models()) {
        Ok(rows) => live_json(serde_json::json!({
            "href": "/index/sizes", "kind": "sized-models",
            "bands": index::SIZE_BANDS.iter().map(|(k, l, lo, hi, says)|
                serde_json::json!({"key": k, "name": l, "from_billions": lo,
                                   "to_billions": if hi.is_infinite() { serde_json::Value::Null }
                                                  else { serde_json::json!(hi) },
                                   "means": says})).collect::<Vec<_>>(),
            "count": rows.len(), "models": rows})),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}

async fn coverage() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let marks = checks_html(&ix.checks()?);
        Ok(coverage_html(&ix.coverage()?, &marks))
    };
    match render() {
        Ok(h) => html(h),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn coverage_json() -> impl IntoResponse {
    match open().and_then(|ix| {
        let mut v = ix.coverage()?;
        v["self_check"] = ix.checks()?;
        Ok(v)
    }) {
        Ok(v) => json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

/// Every list the catalogue keeps, and how many are in each. A reader who
/// arrives on one list has no way of knowing what else is cut this way; this
/// is that map, and it is generated, so a list cannot be built and forgotten.
async fn lists() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let mut sections: Vec<String> = Vec::new();
        let mut total = 0usize;
        let dollar = ix.dollar_models(1_000_000)?.len();

        let mut group = |heading: &str, blurb: &str, rows: Vec<(String, String, usize)>| {
            if rows.is_empty() {
                return;
            }
            total += rows.len();
            let items: String = rows
                .iter()
                .map(|(href, label, n)| {
                    format!(
                        "<li><a href=\"{}\"><span class=\"nm\">{}</span>\
                         <span class=\"sc\">{}</span></a></li>",
                        esc(href),
                        esc(label),
                        grouped(*n as i64)
                    )
                })
                .collect();
            sections.push(format!(
                "<h2>{} <span class=\"n\">{} lists</span></h2><p class=\"intro\">{}</p>\
                 <ul class=\"rows plain\">{items}</ul>",
                esc(heading),
                rows.len(),
                esc(blurb)
            ));
        };

        let count = |page: &Option<Value>| -> usize {
            page.as_ref()
                .and_then(|p| p["members"].as_array().map(|m| m.len()))
                .unwrap_or(0)
        };

        let mut rows = Vec::new();
        for t in ix.task_tags()? {
            let slug = index::address_slug(&t);
            let page = ix.list_page(&[("for", &slug)])?;
            if page.is_some() {
                rows.push((format!("/index/for/{slug}"), task_words(&t), count(&page)));
            }
        }
        rows.sort_by(|a, b| b.2.cmp(&a.2));
        // Two lists somebody wrote by hand, because the question they answer
        // is not a facet of anything: the picks are a judgement, and the
        // dollar list is a threshold.
        group("Chosen rather than filtered",
              "Two lists nobody could derive from a column. One says which model \
               is best at each kind of work; the other says which of the measured \
               ones you can run for almost nothing.",
              vec![
                  ("/index/top".to_string(), "The picks".to_string(), 14),
                  ("/index/bang".to_string(), "Bang for the buck".to_string(), 4),
                  ("/index/1dollar".to_string(), "$1 models".to_string(), dollar),
              ]);

        group("What a thing is for",
              "The job somebody wants done. Read off the modality, the maker's own \
               description and the boards a thing is measured on — never guessed at.",
              rows);

        let mut rows = Vec::new();
        for (fam, what, _) in index::LICENCE_FAMILIES {
            let page = ix.list_page(&[("licence", fam)])?;
            if page.is_some() {
                rows.push((format!("/index/licence/{fam}"), what.to_string(), count(&page)));
            }
        }
        group("What you may do with it",
              "Whether the weights are published, and what the licence asks in return. \
               Every one read off a model card rather than inferred.",
              rows);

        let mut rows = Vec::new();
        for (band, gb, device) in index::MEMORY_BANDS {
            let page = ix.list_page(&[("local", band)])?;
            if page.is_some() {
                rows.push((
                    format!("/index/local/{band}"),
                    format!("{} GB — {device}", *gb as i64),
                    count(&page),
                ));
            }
        }
        group("What your own machine can hold",
              "Open-weight models small enough to run yourself, by the memory of the \
               device. Four-bit weights, total parameters counted, arithmetic stated \
               on each page.",
              rows);

        let mut rows = Vec::new();
        for (i, o) in ix.modality_pairs()? {
            let slug = format!("{}-to-{}", index::address_slug(&i), index::address_slug(&o));
            let page = ix.list_page(&[("does", &slug)])?;
            if count(&page) >= 5 {
                rows.push((
                    format!("/index/does/{slug}"),
                    format!("{i} → {o}"),
                    count(&page),
                ));
            }
        }
        rows.sort_by(|a, b| b.2.cmp(&a.2));
        group("What goes in and what comes out",
              "The shape of a thing, which decides whether it can be used at all. \
               Lists of fewer than five are left off this map but still answer.",
              rows);

        let mut rows = Vec::new();
        for (hub, register) in [("models", "model"), ("tools", "tool"), ("agents", "agent"),
                                ("subscriptions", "subscription")] {
            let n: i64 = ix.count_register(register)?;
            rows.push((format!("/index/{hub}"), format!("every {register}"), n as usize));
        }
        rows.push((
            "/index/providers".into(),
            "every company".into(),
            ix.provider_addresses()?
                .into_iter()
                .filter(|(id, _, _)| !ix.provider_is_empty(id).unwrap_or(false))
                .count(),
        ));
        group("Everything, in order",
              "The whole catalogue, a hundred to a page, alphabetically.",
              rows);

        let n_people = ix.people().map(|p| p.len()).unwrap_or(0);
        group("Around the market",
              "The people behind the companies, and the wire the crawler reads.",
              vec![
                  ("/index/people".to_string(), "People".to_string(), n_people),
                  ("/index/news".to_string(), "News".to_string(), 100),
              ]);

        let rows: Vec<(String, String, usize)> = ix
            .board_counts()?
            .into_iter()
            .map(|(suite, name, n)| {
                (format!("/index/board/{}", index::address_slug(&suite)), name, n as usize)
            })
            .collect();
        group("Who is measured, and where",
              "Public leaderboards the catalogue reads. The count is how many of the \
               models each board ranks are things the catalogue also holds.",
              rows);

        let lede = format!(
            "{} lists, each one a way of cutting the catalogue that somebody would \
             actually search for. Any two of the first three can be crossed — \
             open-weight voice models, coding models that fit in 32 GB — and every \
             crossing that has a member has its own address.",
            grouped(total as i64)
        );
        Ok(shell(
            "Every list",
            &lede,
            "/index/lists",
            format!("<h1>Every list</h1><p class=\"lede\">{}</p>{}", esc(&lede), sections.join("")),
            String::new(),
        ))
    };
    match render() {
        Ok(page) => html(page),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

/// The same map, for a machine deciding which list to read.
async fn lists_json() -> impl IntoResponse {
    let render = || -> anyhow::Result<Value> {
        let ix = open()?;
        let mut out = Vec::new();
        for t in ix.task_tags()? {
            let slug = index::address_slug(&t);
            if let Some(p) = ix.list_page(&[("for", &slug)])? {
                out.push(serde_json::json!({"axis": "for", "value": t,
                    "href": format!("/index/for/{slug}"),
                    "members": p["members"].as_array().map(|m| m.len()).unwrap_or(0)}));
            }
        }
        for (fam, what, _) in index::LICENCE_FAMILIES {
            if let Some(p) = ix.list_page(&[("licence", fam)])? {
                out.push(serde_json::json!({"axis": "licence", "value": fam, "means": what,
                    "href": format!("/index/licence/{fam}"),
                    "members": p["members"].as_array().map(|m| m.len()).unwrap_or(0)}));
            }
        }
        for (band, gb, device) in index::MEMORY_BANDS {
            if let Some(p) = ix.list_page(&[("local", band)])? {
                out.push(serde_json::json!({"axis": "local", "value": band, "gb": gb,
                    "device": device, "href": format!("/index/local/{band}"),
                    "members": p["members"].as_array().map(|m| m.len()).unwrap_or(0)}));
            }
        }
        for (i, o) in ix.modality_pairs()? {
            let slug = format!("{}-to-{}", index::address_slug(&i), index::address_slug(&o));
            if let Some(p) = ix.list_page(&[("does", &slug)])? {
                out.push(serde_json::json!({"axis": "does", "value": format!("{i} -> {o}"),
                    "href": format!("/index/does/{slug}"),
                    "members": p["members"].as_array().map(|m| m.len()).unwrap_or(0)}));
            }
        }
        for (suite, name, n) in ix.board_counts()? {
            out.push(serde_json::json!({"axis": "board", "value": name,
                "href": format!("/index/board/{}", index::address_slug(&suite)),
                "members": n}));
        }
        Ok(serde_json::json!({"kind": "lists", "href": "/index/lists", "lists": out}))
    };
    match render() {
        Ok(v) => json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

/// One recommendation, in the row the rest of the catalogue is set in: what
/// it is called and what it costs on the first line, who made it and why it
/// was chosen on the second. The label rail on the left is the only addition,
/// because here the reader is choosing between four answers rather than
/// scanning one list.
fn pick_html(p: &Value, cells: &[String], leads: &[String]) -> String {
    let cost = p["cost"].as_f64().unwrap_or(0.0);
    let unit = index::top::unit_words(p["unit"].as_str().unwrap_or(""));
    let rank = p["rank"].as_i64().unwrap_or(0);
    let field = p["field"].as_i64().unwrap_or(0);
    let boards = p["boards"].as_i64().unwrap_or(0);
    let sellers = p["sellers"].as_i64().unwrap_or(0);
    let maker = p["maker"].as_str().unwrap_or("");
    // The reason, in the order a person would give it: where it stands, how
    // widely it was asked, and who made it. Never a bare percentile — "94th"
    // means nothing without the field it is 94th of.
    let mut why = vec![format!(
        "{} of {} on {}",
        ordinal(rank), field, esc(p["board"].as_str().unwrap_or(""))
    )];
    if boards > 1 {
        why.push(format!("{boards} boards"));
    }
    if !maker.is_empty() {
        why.insert(0, esc(maker));
    }
    // Sellers are the one figure no leaderboard carries: how easily you could
    // leave. One seller is a hostage, twelve is a market.
    let leave = match sellers {
        0 => String::new(),
        1 => "<span class=\"sc one\">1 seller</span>".to_string(),
        n => format!("<span class=\"sc\">{n} sellers</span>"),
    };
    // A token model is bought at two prices and a reader budgets against
    // both. The blended figure is what ordered the list; quoting it as the
    // price would be quoting a number nobody is ever charged.
    let (figure, un) = match (p["in"].as_f64(), p["out"].as_f64()) {
        (Some(i), Some(o)) => (
            format!("{}<span class=\"to\">→</span>{}",
                    money((i * 1e6) as i64), money((o * 1e6) as i64)),
            "per Mtok in / out".to_string(),
        ),
        _ => (money((cost * 1e6) as i64), unit.to_string()),
    };
    format!(
        "<li><a href=\"{href}\" title=\"{lead}\">\
         <span class=\"cell\">{label}</span>\
         <span class=\"nm\">{name}</span>\
         <span class=\"mk\">{why}</span>\
         <span class=\"pr\">{figure}<span class=\"un\">{un}</span></span>\
         {leave}</a></li>",
        href = esc(p["href"].as_str().unwrap_or("#")),
        lead = esc(&leads.join(", or ")),
        label = esc(&cells.join(" · ")),
        name = esc(p["name"].as_str().unwrap_or("")),
        why = why.join(" · "),
        un = esc(&un),
    )
}

/// A niche and its picks. When one model wins more than one way its cells
/// are merged rather than printed twice — four copies of a name reads like a
/// bug, and "best and cheapest" is a stronger sentence than either alone.
fn niche_html(v: &Value, linked: bool) -> String {
    let key = v["key"].as_str().unwrap_or("");
    let empty = Vec::new();
    let picks = v["picks"].as_array().unwrap_or(&empty);
    let mut order: Vec<String> = Vec::new();
    let mut cells: std::collections::HashMap<String, (Vec<String>, Vec<String>, Value)> =
        std::collections::HashMap::new();
    for p in picks {
        let id = p["entity"].as_str().unwrap_or("").to_string();
        let cell = match p["cell"].as_str().unwrap_or("") {
            "value" => "Best value for money",
            "frontier" => "Best frontier",
            "open" => "Best open source",
            "cheapest" => "Cheapest",
            other => other,
        };
        let lead = p["lead"].as_str().unwrap_or("").to_string();
        let e = cells.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            (Vec::new(), Vec::new(), p.clone())
        });
        e.0.push(cell.to_string());
        e.1.push(lead);
    }
    let body: String = order
        .iter()
        .filter_map(|id| cells.get(id))
        .map(|(c, l, p)| pick_html(p, c, l))
        .collect();
    let title = esc(v["title"].as_str().unwrap_or(""));
    let heading = if linked {
        format!("<a href=\"/index/top/{key}\">{title}</a>")
    } else {
        title
    };
    let measured = v["measured"].as_i64().unwrap_or(0);
    let buyable = v["buyable"].as_i64().unwrap_or(0);
    let eligible = v["eligible"].as_i64().unwrap_or(buyable);
    let question = v["question"].as_str().unwrap_or("");
    // Say why the pool is the size it is. "19 of them you can buy" was false
    // when 61 were for sale and 19 had been measured twice — the reader would
    // have read a corroboration rule as a shortage of sellers.
    let sub = if buyable == 0 {
        format!("{}. Measured, but nobody sells it at a comparable price yet.",
                esc(&sentence(question)))
    } else if eligible < buyable {
        format!("{}. {} measured, {} of them for sale, and {} of those stand on two \
                 or more of these boards — only those can be picked.",
                esc(&sentence(question)), grouped(measured), grouped(buyable), grouped(eligible))
    } else {
        format!("{}. {} measured, {} of them you can buy today.",
                esc(&sentence(question)), grouped(measured), grouped(buyable))
    };
    // On its own page the niche already has the h1 and the lede above it;
    // repeating them as a section header says the same thing three times.
    let head = if linked {
        format!("<div class=\"nh\"><h3>{heading}</h3><p class=\"q\">{sub}</p></div>")
    } else {
        format!("<p class=\"q solo\">{sub}</p>")
    };
    format!("<section class=\"niche\" id=\"{key}\">{head}<ul class=\"rows picks\">{body}</ul></section>")
}

/// The opening sentence, with the heading's own words taken off the front.
/// "Writing code models, tools and agents" under a heading that already says
/// "Writing code models, tools and agents" is a page that looks unfinished.
/// The full sentence still goes to the search snippet, where the repetition
/// is what a search engine wants.
fn under(title: &str, lede: &str) -> String {
    let t = title.trim_end_matches('.');
    match lede.strip_prefix(t) {
        Some(rest) => {
            let rest = rest.trim_start_matches(['.', ',', ':', ' ', '—', '–']);
            let mut c = rest.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => lede.to_string(),
            }
        }
        None => lede.to_string(),
    }
}

/// A question written as a sentence: the table stores "which model draws the
/// picture" so it reads correctly in both a heading and a paragraph.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

const TOP_LEDE: &str = "For every kind of AI work: the best value for money, the \
best there is, the best with open weights where there is one, and the cheapest \
that still works. Each is a single ranking with a single stated rule, rebuilt \
every night from the boards and the price lists.";

async fn top_all() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let niches = ix.top_index()?;
        // Grouped into families, in the families' own order, so the page
        // reads as five subjects rather than fourteen headings.
        let mut body = String::new();
        let mut jump = String::new();
        for (family, blurb) in index::top::FAMILIES {
            let mine: Vec<&Value> = niches
                .iter()
                .filter(|n| n["family"].as_str() == Some(*family))
                .filter(|n| !n["picks"].as_array().map_or(true, |a| a.is_empty()))
                .collect();
            if mine.is_empty() {
                continue;
            }
            body.push_str(&format!(
                "<div class=\"fam\"><h2>{}</h2><p class=\"fb\">{}</p>{}</div>",
                esc(family),
                esc(blurb),
                mine.iter().map(|n| niche_html(n, true)).collect::<String>()
            ));
            jump.push_str(&format!(
                "<span class=\"jg\"><b>{}</b>{}</span>",
                esc(family),
                mine.iter()
                    .map(|n| format!("<a href=\"/index/top/{}\">{}</a>",
                                     n["key"].as_str().unwrap_or(""),
                                     esc(n["title"].as_str().unwrap_or(""))))
                    .collect::<String>()
            ));
        }
        let read = ix.last_read()?;
        Ok(shell(
            "The best model for every job",
            TOP_LEDE,
            "/index/top",
            format!(
                "<h1>The picks</h1><p class=\"lede\">{TOP_LEDE}</p>\
                 <nav class=\"jump\">{jump}</nav>{body}\
                 <p class=\"note\">Every pick has to be something you can buy today: \
                 a model with no seller and no published price is not an answer to \
                 what should I use. Prices are the standard lane, never a batch or \
                 promotional rate. Figures read {read}.</p>"
            ),
            String::new(),
        ))
    };
    match render() {
        Ok(h) => html(h),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn top_all_json() -> impl IntoResponse {
    let render = || -> anyhow::Result<Value> {
        let ix = open()?;
        Ok(serde_json::json!({
            "kind": "top-index", "href": "/index/top",
            "read": ix.last_read()?, "niches": ix.top_index()?,
        }))
    };
    match render() {
        Ok(v) => json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

/// One niche on its own page, so "best open source coding model" has an
/// address of its own — which is what somebody types.
fn top_html(v: &Value, read: &str) -> String {
    let title = v["title"].as_str().unwrap_or("");
    let key = v["key"].as_str().unwrap_or("");
    let question = sentence(v["question"].as_str().unwrap_or(""));
    let boards: String = v["boards"]
        .as_array()
        .map(|b| {
            b.iter()
                .map(|x| {
                    format!("<li><a href=\"{}\">{}</a></li>",
                            esc(x["href"].as_str().unwrap_or("")),
                            esc(x["name"].as_str().unwrap_or("")))
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let lede = format!(
        "{question}. Four answers — best value for money, best there is, best \
         with open weights, cheapest that still works."
    );
    let median = v["median_price"].as_f64().unwrap_or(0.0);
    let unit = index::top::unit_words(v["unit"].as_str().unwrap_or(""));
    let how = if median > 0.0 {
        format!(
            "<p class=\"note\">Capability is a model's mean percentile across the \
             boards below, on each board's widest metric. Prices are quoted as \
             you are billed them; where a model charges separately to read and \
             to write, the two are ranked against each other at one part in to \
             three parts out, which for this field puts the middle at {} \
             {unit}. <em>Best value for money</em> is the most capable at or \
             under that middle, and <em>cheapest</em> is the least expensive of \
             those above the middle for capability — without that second floor \
             the cheapest answer is reliably the worst model in the category. \
             Figures read {read}.</p>",
            money((median * 1e6) as i64)
        )
    } else {
        format!("<p class=\"note\">Figures read {read}.</p>")
    };
    // A board kept only because the niche has no other is weak evidence, and
    // the page has to say so where the pick is read, not in a footnote.
    let crowded = match v["crowded"].as_array() {
        Some(c) if !c.is_empty() => format!(
            "<p class=\"note\">{} is kept for want of a better one: its entrants sit within \
             a few per cent of each other, so whoever leads it leads by very little. Read \
             the pick above as the best of a crowd rather than a clear winner.</p>",
            c.iter().filter_map(|x| x.as_str()).map(esc).collect::<Vec<_>>().join(" and ")
        ),
        _ => String::new(),
    };
    // The heading is one word on purpose; the title cannot be, because
    // "Agents" is already the name of the hub and "Voice" is the name of a
    // product. Two pages with one title compete for the same reader.
    shell(
        &format!("{title} — the picks"),
        &lede,
        &format!("/index/top/{key}"),
        format!(
            "<h1>{}</h1><p class=\"lede\">{}</p>{}\
             <h2>Judged on</h2><ul class=\"rows plain boards\">{boards}</ul>{crowded}{how}\
             <p class=\"note\"><a href=\"/index/top\">All the picks</a></p>",
            esc(title),
            esc(&lede),
            niche_html(v, false)
        ),
        String::new(),
    )
}

/// Line icons for the About block. Drawn rather than fetched: the page is a
/// few kilobytes and must stay that way, and a sprite that cannot load is a
/// row that reads as a bug.
fn icon(key: &str) -> String {
    let d = match key {
        "maker" => "M3 20h18M5 20V9l7-5 7 5v11M10 20v-5h4v5",
        "kind" => "M4 7h16M4 12h16M4 17h10",
        "in" => "M12 19V5M5 12l7-7 7 7",
        "out" => "M12 5v14M5 12l7 7 7-7",
        "context" => "M4 6h16v12H4zM8 6v12M16 6v12",
        "size" => "M4 12h16M8 8l-4 4 4 4M16 8l4 4-4 4",
        "licence" => "M6 3h9l3 3v15H6zM13 3v4h4M9 12h6M9 16h6",
        "sellers" => "M4 20v-6a4 4 0 0 1 8 0v6M8 7a3 3 0 1 0 0-.1M16 20v-5a4 4 0 0 0-2-3.4",
        "price" => "M12 3v18M8 7h6a3 3 0 0 1 0 6H9a3 3 0 0 0 0 6h6",
        "board" => "M4 20V10M10 20V4M16 20v-8M22 20H2",
        "link" => "M10 14a4 4 0 0 0 6 .5l2-2a4 4 0 0 0-6-6l-1 1M14 10a4 4 0 0 0-6-.5l-2 2a4 4 0 0 0 6 6l1-1",
        _ => "M12 3a9 9 0 1 0 .1 0M12 8v5M12 16h.01",
    };
    // Owned, not leaked: this is called once per figure per request, and
    // Box::leak handed every one of those strings to the allocator forever —
    // a slow per-render memory leak on a hot path.
    format!("<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"{d}\"/></svg>")
}

/// The About block: a line you can quote, a paragraph of what it is, and every
/// current figure in a form that can be copied whole.
fn about_html(a: &index::about::About, read: &str, canonical: &str, title: &str) -> String {
    if a.line.is_empty() {
        return String::new();
    }
    let rows: String = a
        .figures
        .iter()
        .map(|(ic, label, value)| {
            format!(
                "<div class=\"abt-fig\">{}<dt>{}</dt><dd>{}</dd></div>",
                icon(ic), esc(label), esc(value)
            )
        })
        .collect();
    // What the copy button adds to the block, rather than a second copy of
    // every figure sitting hidden in the markup: that cost a kilobyte on
    // every card and pushed fifty pages over the weight a phone should be
    // asked to carry.
    let tail = format!("{title}|{read}|https://pass.io{canonical}");

    let para = if a.paragraph.is_empty() {
        String::new()
    } else {
        format!("<p class=\"apara\">{}</p>", esc(&a.paragraph))
    };
    format!(
        "<section class=\"about\">\
         <div class=\"ahead\"><h2>About</h2>\
          <button class=\"cp\" type=\"button\" data-copy=\"line\">Copy the line</button></div>\
         <p class=\"aline\"><span>{}</span></p>\
         {para}\
         <div class=\"afig\"><div class=\"ahead\"><h3>Every current figure</h3>\
          <button class=\"cp\" type=\"button\" data-copy=\"figs\">Copy all</button></div>\
          <dl class=\"abt-figs\" data-tail=\"{}\">{rows}</dl></div></section>",
        esc(&a.line),
        esc(&tail)
    )
}

const COPY_JS: &str = r#"document.addEventListener('click',function(e){
var b=e.target.closest('.cp');if(!b)return;
var s=b.closest('section'),t;
if(b.dataset.copy==='line'){t=s.querySelector('.aline span').textContent}
else{var d=s.querySelector('.abt-figs'),x=d.dataset.tail.split('|'),L=[x[0]];
d.querySelectorAll('.abt-fig').forEach(function(f){
L.push((f.querySelector('dt').textContent+'            ').slice(0,12)+' '+f.querySelector('dd').textContent)});
L.push('Read         '+x[1]);L.push('Source       '+x[2]);t=L.join('\n')}
navigator.clipboard.writeText(t).then(function(){
var o=b.textContent;b.textContent='Copied';b.classList.add('done');
setTimeout(function(){b.textContent=o;b.classList.remove('done')},1400)})});"#;

const FREE_LEDE: &str = "Things somebody will run for you and charge you \
nothing: free tiers, free endpoints, free plans. Not open weights — that you \
may download and host a model yourself is a fact about its licence, and the \
licence lists already say it. This page is only what costs you nothing and \
takes no machine of your own.";

/// What can be had without paying, and the catch on each kind.
async fn free_of(Path(kind): Path<String>) -> impl IntoResponse {
    let (kind, as_json) = match kind.strip_suffix(".json") {
        Some(k) => (k.to_string(), true),
        None => (kind, false),
    };
    let register = match kind.as_str() {
        "models" => "model",
        "tools" => "tool",
        "agents" => "agent",
        "subscriptions" => "subscription",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let render = || -> anyhow::Result<axum::response::Response> {
        let ix = open()?;
        let p = ix.free_page()?;
        let empty = Vec::new();
        let list: Vec<Value> = p["called"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|x| x["register"].as_str() == Some(register))
            .cloned()
            .collect();
        if as_json {
            return Ok(json(serde_json::json!({
                "kind": "free", "of": register, "href": format!("/index/free/{kind}"),
                "read": p["read"], "members": list,
            })));
        }
        let rich = rich_rows(&ix)?;
        let (heading, blurb) = free_words(register);
        let rows: String = list.iter().map(|m| free_row(m, &rich)).collect();
        let read = p["read"].as_str().unwrap_or("");
        let lede = format!(
            "{} you can use without paying anyone — {} of them, each with the allowance \
             its seller published.",
            heading, list.len()
        );
        Ok(html(shell(
            &format!("Free {}", heading.to_lowercase()),
            &lede,
            &format!("/index/free/{kind}"),
            format!(
                "<h1>Free {}</h1><p class=\"lede\">{}</p>\
                 <p class=\"intro\">{}</p><ul class=\"rows plain\">{rows}</ul>{}\
                 <p class=\"note\">Figures read {read}. \
                  <a href=\"/index/free\">Everything that is free</a>.</p>",
                esc(&heading.to_lowercase()),
                esc(&lede),
                esc(blurb),
                daily_free(&list)
            ),
            String::new(),
        )))
    };
    match render() {
        Ok(r) => r,
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

/// What each kind of free thing is, said once and used on both the overview
/// and the page of its own.
fn free_words(register: &str) -> (&'static str, &'static str) {
    match register {
        "model" => ("Models",
            "A model somebody will run for you at no charge. The allowance is the entry:              thirty requests a minute is generous for a side project and nothing at all              for a product, and which it is depends on what you are building."),
        "tool" => ("Tools",
            "Search, extraction, transcription. Tools carry free tiers as readily as              models do and are almost never listed beside them."),
        "agent" => ("Agents",
            "Something that will go and do the work. Rarer than a free model, and the              quota is usually the reason."),
        _ => ("Subscriptions",
            "An allowance rather than a rate: a monthly credit, a daily ceiling, or a              tier of a paid product that is given away. Each says what it withholds."),
    }
}

/// What a day of free access is worth across a whole block.
///
/// A monthly allowance is divided by thirty and folded into the daily figure,
/// because a reader comparing what they can have today does not care which
/// calendar a seller bills on. That division is an estimate and the page says
/// so: nobody published the daily number, we worked it out. A credit handed
/// over once on signing up is not divided — it does not come back next month,
/// and spreading it over days would suggest it did.
///
/// An allowance whose ceiling the seller does not publish — Google's, which
/// lives inside a signed-in console — is counted as unstated rather than as
/// nothing, and the count is printed.
fn daily_free(list: &[Value]) -> String {
    let (mut rpd, mut tpd, mut neurons) = (0f64, 0f64, 0f64);
    let (mut credits_d, mut usd_d) = (0f64, 0f64);
    let (mut usd_once, mut credits_once) = (0f64, 0f64);
    let (mut counted, mut unstated, mut monthly) = (0usize, 0usize, 0usize);
    let empty = Vec::new();
    for m in list {
        for f in m["from"].as_array().unwrap_or(&empty) {
            let a = &f["allowance"];
            let n = |k: &str| a[k].as_f64().unwrap_or(0.0);
            if !a.as_object().map_or(false, |o| !o.is_empty()) {
                unstated += 1;
                continue;
            }
            counted += 1;
            rpd += n("rpd");
            tpd += n("tpd");
            neurons += n("neurons_day");
            if n("calls_month") > 0.0 || n("credits_month") > 0.0 || n("usd_month") > 0.0 {
                monthly += 1;
            }
            rpd += n("calls_month") / 30.0;
            credits_d += n("credits_month") / 30.0;
            usd_d += n("usd_month") / 30.0;
            usd_once += n("usd_once");
            credits_once += n("credits_once");
        }
    }
    let round = |v: f64| grouped(v.round() as i64);
    let mut day: Vec<String> = Vec::new();
    if rpd >= 1.0 {
        day.push(format!("<b>{}</b> requests", round(rpd)));
    }
    if tpd >= 1.0 {
        day.push(format!("<b>{}</b> tokens", round(tpd)));
    }
    if credits_d >= 1.0 {
        day.push(format!("<b>{}</b> credits", round(credits_d)));
    }
    if neurons >= 1.0 {
        day.push(format!("<b>{}</b> Neurons", round(neurons)));
    }
    if usd_d > 0.0 {
        day.push(format!("<b>${:.2}</b> of credit", usd_d));
    }
    let mut once: Vec<String> = Vec::new();
    if usd_once > 0.0 {
        once.push(format!("${:.0} of credit", usd_once));
    }
    if credits_once >= 1.0 {
        once.push(format!("{} credits", round(credits_once)));
    }
    if day.is_empty() && once.is_empty() {
        return String::new();
    }
    let mut parts = String::new();
    if !day.is_empty() {
        parts.push_str(&format!("<span class=\"a day\">{}</span>", day.join(" · ")));
    }
    if !once.is_empty() {
        parts.push_str(&format!(
            "<span class=\"a\">and {} once, on signing up</span>", once.join(" · ")
        ));
    }
    let est = if monthly > 0 {
        format!(
            " {} of them are stated by the month and divided by thirty to get here, so \
             this figure is an estimate rather than something anybody published.",
            grouped(monthly as i64)
        )
    } else {
        String::new()
    };
    let gap = if unstated > 0 {
        format!(
            " Another {} publish no ceiling at all, so nothing of theirs is in the total.",
            grouped(unstated as i64)
        )
    } else {
        String::new()
    };
    format!(
        "<div class=\"tally\"><h3>Free every day, all of them together</h3>\
         <div class=\"amts\">{parts}</div>\
         <p class=\"why\">Added from what {} of these sellers state as a figure.{est}{gap}</p>\
         </div>",
        grouped(counted as i64)
    )
}

/// One row of a free listing, with the seller and the allowance where a price
/// would go — on this page the allowance is the price.
fn free_row(m: &Value, rich: &HashMap<String, Value>) -> String {
    let id = m["href"].as_str().unwrap_or("");
    let mut row = rich.get(id).cloned().unwrap_or_else(|| {
        serde_json::json!({"n": m["name"], "h": m["href"], "r": "model"})
    });
    row["p"] = Value::Null;
    row["o"] = Value::Null;
    row["lm"] = Value::Null;
    // Everybody who hands this one out, each with what they allow. Two
    // sellers is two lines inside one row, not two rows.
    let empty = Vec::new();
    let extra: String = m["from"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|f| {
            let seller = esc(f["seller"].as_str().unwrap_or(""));
            match f["terms"].as_str().unwrap_or("") {
                "" => format!("<span class=\"gift\"><b>{seller}</b></span>"),
                t => format!("<span class=\"gift\"><b>{seller}</b> {}</span>", esc(t)),
            }
        })
        .collect();
    row_html(&row).replace("</a></li>", &format!("<span class=\"terms\">{extra}</span></a></li>"))
}

async fn free() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let p = ix.free_page()?;
        let rich = rich_rows(&ix)?;
        let empty = Vec::new();

        // Grouped by what the thing is, because "free" reads very differently
        // on a model, on a tool and on a plan, and a flat list of forty-two
        // hides that tools have free tiers as readily as models do.
        let of = |register: &str| -> Vec<Value> {
            p["called"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter(|x| x["register"].as_str() == Some(register))
                .cloned()
                .collect()
        };

        let seen = |register: &str, at: &str| -> String {
            let list = of(register);
            if list.is_empty() {
                return String::new();
            }
            let (heading, blurb) = free_words(register);
            let rows: String = list.iter().map(|m| free_row(m, &rich)).collect();
            format!(
                "<section class=\"freeg\"><h2><a href=\"/index/free/{at}\">{}</a> \
                 <span class=\"n\">{}</span></h2>\
                 <p class=\"intro\">{}</p><ul class=\"rows plain\">{rows}</ul>{}</section>",
                esc(heading), grouped(list.len() as i64), esc(blurb), daily_free(&list)
            )
        };
        let models = seen("model", "models");
        let tools = seen("tool", "tools");
        let agents = seen("agent", "agents");
        let plans = seen("subscription", "subscriptions");
        // Weights you may download are not what this page is for. That you
        // can run an open model yourself is a property of its licence, it is
        // already a list of its own, and putting 284 of them here buried the
        // fifteen things somebody will actually serve you for nothing.
        // A licence that stops being free the moment the thing you build earns
        // is not a free thing; it is a trial with a lawyer attached. Listing
        // it here would be the page's one dishonest row.

        let read = p["read"].as_str().unwrap_or("");
        Ok(shell(
            "Everything you can use for nothing",
            FREE_LEDE,
            "/index/free",
            format!(
                "<h1>Free</h1><p class=\"lede\">{FREE_LEDE}</p>{models}{tools}{agents}{plans}\
                 <p class=\"note\">Nothing here is inferred. A rate of nought counts only \
                 where the seller published it as one — everywhere else in the catalogue a \
                 nought is treated as a rounding mistake and blocked, because a price that \
                 rounded below a micro-dollar is not a gift. Figures read {read}.</p>"
            ),
            String::new(),
        ))
    };
    match render() {
        Ok(h) => html(h),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn free_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.free_page()) {
        Ok(v) => json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

const WAITING_LEDE: &str = "Companies that sell by conversation. Some of the largest \
firms in this market quote per engagement rather than off a rate card, which is how \
enterprise software has always been bought. Below them are names the market mentioned \
once and nobody has priced yet; the order says which is which.";

/// B2B: what is sold by conversation rather than off a price list, and below it
/// the names nobody has priced yet.
async fn waiting() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let w = ix.waiting()?;
        let empty = Vec::new();
        let companies = w["companies"].as_array().unwrap_or(&empty);
        let rows: String = companies
            .iter()
            .map(|c| {
                let makes = c["makes"].as_i64().unwrap_or(0);
                let said = c["said"].as_i64().unwrap_or(0);
                // A reason somebody wrote outranks any count. The
                // significance of a company that publishes no price cannot be
                // computed from a catalogue of prices, so where it matters it
                // is written down and signed.
                let weight = match (c["why"].as_str().unwrap_or(""), makes, said) {
                    (w, _, _) if !w.is_empty() => esc(w),
                    _ => String::new(),
                };
                let weight = if !weight.is_empty() { weight } else { match (makes, said) {
                    (0, 0) => "nobody has said why this name is here".to_string(),
                    (m, 0) => format!("{} here name it as their maker", plural_n(m, "thing")),
                    (0, s) => format!("written about by {}", plural_n(s, "source")),
                    (m, s) => format!(
                        "{} name it as their maker, written about by {}",
                        plural_n(m, "thing"), plural_n(s, "source")
                    ),
                }};
                let home = match c["url"].as_str().unwrap_or("") {
                    "" => String::new(),
                    u => format!(
                        "<span class=\"sc\"><a href=\"{}\" rel=\"nofollow noreferrer\">{}</a></span>",
                        safe_href(u), esc(u.trim_start_matches("https://").trim_start_matches("http://"))
                    ),
                };
                // A name nobody has justified gets no link: its page holds
                // nothing, it is not in the sitemap, and pointing a reader at
                // it would be the catalogue wasting their click on its own
                // housekeeping.
                let named = esc(c["name"].as_str().unwrap_or(""));
                let body = format!(
                    "<span class=\"nm\">{named}</span>\
                     <span class=\"mk\">{} · {}</span>",
                    esc(c["kind"].as_str().unwrap_or("")),
                    esc(&weight)
                );
                if c["why"].as_str().unwrap_or("").is_empty() && makes + said == 0 {
                    format!("<li class=\"unjustified\">{body}</li>")
                } else {
                    format!(
                        "<li><a href=\"/index/{}\">{body}</a>{home}</li>",
                        index::address_slug(c["name"].as_str().unwrap_or(""))
                    )
                }
            })
            .collect();
        let unbound = w["unbound"].as_array().unwrap_or(&empty);
        let ub: String = unbound
            .iter()
            .map(|u| {
                format!(
                    "<li><span class=\"nm\">{}</span><span class=\"mk\">from {}</span></li>",
                    esc(u["alias"].as_str().unwrap_or("")),
                    esc(u["source"].as_str().unwrap_or(""))
                )
            })
            .collect();
        let bare = w["bare"].as_array().unwrap_or(&empty);
        let br: String = bare
            .iter()
            .map(|b| {
                format!(
                    "<li><span class=\"nm\">{}</span><span class=\"mk\">{}</span></li>",
                    esc(b["name"].as_str().unwrap_or("")),
                    esc(b["register"].as_str().unwrap_or(""))
                )
            })
            .collect();
        let read = w["read"].as_str().unwrap_or("");
        Ok(shell(
            "B2B — sold, but not off a price list",
            WAITING_LEDE,
            "/index/waiting",
            format!(
                "<h1>B2B</h1><p class=\"lede\">{WAITING_LEDE}</p>\
                 <h2>Companies that price on request <span class=\"n\">{}</span></h2>\
                 <p class=\"intro\">Ordered by how much of the rest of the catalogue points \
                  Kept on purpose first, each with the reason somebody wrote for keeping \
                  it — Harvey and Sierra are among the largest companies in this market and \
                  sell by contract, which no amount of crawling will change. Then the ones \
                  the rest of the catalogue points at. Last, the names nobody has justified, \
                  which are candidates for removal rather than work.</p>\
                 <ul class=\"rows plain\">{rows}</ul>\
                 <h2>Being identified <span class=\"n\">{}</span></h2>\
                 <p class=\"intro\">A seller published something and the catalogue could not \
                  tell what it was. This is not a gap in the market; it is a gap in our \
                  reading of it, and it is the operator's queue.</p>\
                 <ul class=\"rows plain\">{ub}</ul>\
                 <h2>Cards with nothing on them <span class=\"n\">{}</span></h2>\
                 <p class=\"intro\">No price, no standing, no description. A name, so far.</p>\
                 <ul class=\"rows plain\">{br}</ul>\
                 <p class=\"note\">Read {read}.</p>",
                grouped(companies.len() as i64),
                grouped(unbound.len() as i64),
                grouped(bare.len() as i64)
            ),
            String::new(),
        ))
    };
    match render() {
        Ok(h) => html(h),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn waiting_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.waiting()) {
        Ok(v) => json(v),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

async fn sitemap() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let mut out = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
        );
        // lastmod only where a true data date exists — the day the page
        // last gained a price, a standing or a document. Search engines key
        // recrawls on it, and a padded date is the one lie they punish.
        let dates = ix.address_dates()?;
        for a in ix.all_addresses()? {
            match dates.get(&a) {
                Some(d) => out.push_str(&format!(
                    "<url><loc>https://pass.io{a}</loc><lastmod>{d}</lastmod></url>\n"
                )),
                None => out.push_str(&format!("<url><loc>https://pass.io{a}</loc></url>\n")),
            }
        }
        out.push_str("</urlset>\n");
        Ok(out)
    };
    match render() {
        Ok(xml) => (
            [
                (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            xml,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

// ---------------------------------------------------------------------------
// The door on the data files
// ---------------------------------------------------------------------------

/// One file asks for an account: the catalogue in bulk. Everything else here
/// is open and stays open — the pages, the search, and every page's own
/// `.json` twin, which is how an agent reads one thing without being handed
/// nine megabytes. Citation is the point; a twin behind a door is a fact
/// nobody quotes.
///
/// What is worth an account is the whole thing at once, because that is the
/// shape somebody takes away and works with instead of reading.
const BEHIND_THE_DOOR: &[&str] = &["/index/pass_index_all.json"];

/// Where to ask whether a session is real. The index cannot answer that on
/// its own — sessions belong to Pass, and a cookie this service checked by
/// itself would be a cookie anybody could write.
fn auth_upstream() -> &'static str {
    static U: OnceLock<String> = OnceLock::new();
    U.get_or_init(|| {
        std::env::var("PASS_INDEX_AUTH")
            .unwrap_or_else(|_| "http://172.17.0.1:8180/console/api/me".into())
    })
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(reqwest::Client::new)
}

/// Answers already given, so a reader walking the catalogue does not cost an
/// upstream call per file. Kept short: somebody who has just signed in should
/// not wait a minute for it, and a session that was revoked should not keep
/// working for one.
fn verdicts() -> &'static Mutex<HashMap<String, (bool, Instant)>> {
    static V: OnceLock<Mutex<HashMap<String, (bool, Instant)>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(HashMap::new()))
}

const VERDICT_HOLDS: Duration = Duration::from_secs(60);

fn session_cookie(req: &Request) -> Option<String> {
    req.headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|c| c.strip_prefix("pass_session="))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

async fn signed_in(token: &str) -> bool {
    if let Ok(seen) = verdicts().lock() {
        if let Some((ok, at)) = seen.get(token) {
            if at.elapsed() < VERDICT_HOLDS {
                return *ok;
            }
        }
    }
    // A session is not enough: the file is for a full account — the email
    // confirmed, and past the queue, so Pass shows this person the console
    // rather than a waiting page. Pass answers both in one call.
    //
    // Pass being unreachable means no session can be shown to be real, so
    // none is taken on trust. Nobody is stranded by that who could have got
    // in a moment earlier: signing in is behind the same door.
    let ok = match client()
        .get(auth_upstream())
        .header(header::COOKIE, format!("pass_session={token}"))
        .timeout(Duration::from_secs(4))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(me) => {
                me["verified"].as_bool().unwrap_or(false)
                    && !me["queue_active"].as_bool().unwrap_or(false)
            }
            Err(_) => false,
        },
        _ => false,
    };
    if let Ok(mut seen) = verdicts().lock() {
        if seen.len() > 10_000 {
            seen.clear();
        }
        seen.insert(token.to_string(), (ok, Instant::now()));
    }
    ok
}

/// A reader in a browser is taken to the door; a program is told where it is.
/// Both carry the file that was asked for, so what somebody came for is known
/// rather than guessed at.
fn ask_to_sign_in(path: &str, wants_html: bool) -> Response {
    let safe: String = path
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let door = format!("/signin?signin={safe}");
    if wants_html {
        return (StatusCode::SEE_OTHER, [(header::LOCATION, door)]).into_response();
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        format!(
            "{{\"error\":\"The catalogue in one file is for a full Pass account: \
sign in, confirm the address in the email, and it downloads.\",\
\"signin\":\"https://pass.io{door}\"}}"
        ),
    )
        .into_response()
}

/// What a file should be called once it is off the server.
///
/// A browser names a saved file after the last piece of its address, so the
/// catalogue arrived on people's machines as `all.json` and every card as
/// `gpt-5.json` — names that say nothing a week later about where they came
/// from, and that collide with whatever else is in the folder.
///
/// A filename is read out of context, so it carries all three facts: whose it
/// is, which product, and which page — so the address carries all three and
/// the saved file simply keeps it: `pass_index_all.json`.
fn saved_as(path: &str) -> String {
    // The address is already named after what the file is, so the last piece
    // of it is the filename. Building one from the path as well produced
    // pass_index_pass_index_all.json.
    path.rsplit('/')
        .next()
        .filter(|t| !t.is_empty())
        .unwrap_or("pass_index.json")
        .to_string()
}

/// Only the download is named. A card's twin is read where it lies by an
/// agent that never saves it, and naming all four thousand of them was noise
/// on every response for a save nobody performs.
fn name_the_file(path: &str, res: &mut Response) {
    if !BEHIND_THE_DOOR.contains(&path) {
        return;
    }
    if let Ok(v) = format!("attachment; filename=\"{}\"", saved_as(path)).parse() {
        res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
}

async fn door(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if !BEHIND_THE_DOOR.contains(&path.as_str()) {
        return next.run(req).await;
    }
    let wants_html = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    match session_cookie(&req) {
        Some(t) if signed_in(&t).await => {
            let mut res = next.run(req).await;
            name_the_file(&path, &mut res);
            res
        }
        _ => ask_to_sign_in(&path, wants_html),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::var("PASS_INDEX_ADDR").unwrap_or_else(|_| "0.0.0.0:8185".into());
    let app = Router::new()
        .route("/index", get(browse))
        .route("/index/find.json", get(find_json))
        .route("/index/waiting", get(waiting))
        .route("/index/waiting.json", get(waiting_json))
        .route("/index/free", get(free))
        .route("/index/free/{kind}", get(free_of))
        .route("/index/free.json", get(free_json))
        .route("/index/top", get(top_all))
        .route("/index/top.json", get(top_all_json))
        .route("/index/lists", get(lists))
        .route("/index/lists.json", get(lists_json))
        .route("/index/coverage", get(coverage))
        .route("/index/quarantine", get(quarantine))
        .route("/index/bang", get(bang))
        .route("/index/bang.json", get(bang_json))
        .route("/index/people", get(people))
        .route("/index/people.json", get(people_json))
        .route("/index/news", get(news))
        .route("/index/news.json", get(news_json))
        .route("/index/1dollar", get(dollar))
        .route("/index/1dollar.json", get(dollar_json))
        .route("/index/startups", get(startups))
        .route("/index/startups.json", get(startups_json))
        .route("/index/sizes", get(sizes))
        .route("/index/sizes.json", get(sizes_json))
        .route("/index/tech", get(tech))
        .route("/index/tech.json", get(tech_json))
        .route("/index/tech/{slug}", get(tech_one))
        .route("/index/coverage.json", get(coverage_json))
        .route("/index/pass_index_all.json", get(all_json))
        .route("/index/sitemap.xml", get(sitemap))
        .route("/index/{head}", get(company))
        .route("/index/{head}/{tail}", get(one))
        .route("/index/{a1}/{v1}/{a2}/{v2}", get(two))
        .route("/healthz", get(|| async { "ok" }))
        .layer(middleware::from_fn(door));
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("pass-index serving on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// The people of the market: who founded each company here, and who runs it.
/// Read from Wikidata by the nightly people job; a person links to their
/// companies, never the other way into a person page — the catalogue sells
/// nothing about people and keeps only these two public facts.
async fn people() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let ix = open()?;
        let rows = ix.people()?;
        let n_people = rows.len();
        let n_companies: usize = {
            let mut seen: Vec<&str> = Vec::new();
            for r in &rows {
                for c in r["companies"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let id = c["id"].as_str().unwrap_or("");
                    if !seen.contains(&id) {
                        seen.push(id);
                    }
                }
            }
            seen.len()
        };
        let body_rows: String = rows
            .iter()
            .map(|r| {
                let links: String = r["companies"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|c| {
                                format!(
                                    "<a href=\"{}\">{}</a> <i>({})</i>",
                                    esc(c["href"].as_str().unwrap_or("#")),
                                    esc(c["name"].as_str().unwrap_or("")),
                                    if c["role"].as_str() == Some("founded_by") {
                                        "founder"
                                    } else {
                                        "chief executive"
                                    },
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" · ")
                    })
                    .unwrap_or_default();
                format!(
                    "<li><span class=\"nm\">{}</span><span class=\"mk\">{links}</span></li>",
                    esc(r["name"].as_str().unwrap_or(""))
                )
            })
            .collect();
        let body = format!(
            "<h1>People</h1><p class=\"lede\">Who founded the companies in this catalogue, \
             and who runs them. Two facts per company, both read from Wikidata off the \
             company's own article — never guessed from prose — and each carries its \
             source. A person here is a link to their companies, nothing more.</p>\
             <div class=\"facts\">\
               <div><b>{}</b><span>people</span></div>\
               <div><b>{}</b><span>companies they founded or run</span></div>\
             </div>\
             <ul class=\"rows people\">{body_rows}</ul>\
             <p class=\"io\"><a href=\"/index/startups\">the startups</a> · \
             <a href=\"/index/providers\">every company</a> · \
             <a href=\"/index/people.json\">this page as JSON</a></p>",
            grouped(n_people as i64),
            grouped(n_companies as i64),
        );
        Ok(shell(
            "People — who founded it, who runs it",
            "The founders and chief executives of the companies in the Pass Index.",
            "/index/people",
            body,
            String::new(),
        ))
    };
    match render() {
        Ok(p) => html(p),
        Err(e) => html(format!("<h1>People</h1><p class=\"none\">{}</p>", esc(&e.to_string()))),
    }
}

async fn people_json() -> impl IntoResponse {
    match open().and_then(|ix| ix.people()) {
        Ok(rows) => live_json(serde_json::json!({
            "href": "/index/people", "kind": "people",
            "count": rows.len(), "people": rows,
        })),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}

/// The wire, as the crawler heard it: AI-market headlines from the last two
/// deliveries, newest first. Served straight off the supplier's files — the
/// catalogue stores nothing, so the page is exactly as fresh as the supplier.
fn news_lines(limit: usize) -> anyhow::Result<Vec<Value>> {
    let dir = std::path::Path::new("/crawler/findings");
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
        .collect();
    files.sort();
    let mut out: Vec<Value> = Vec::new();
    for path in files.iter().rev().take(2) {
        let body = std::fs::read_to_string(path)?;
        for line in body.lines() {
            let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
            let lane = d["lane"].as_str().unwrap_or("");
            let what = d["what"].as_str().unwrap_or("");
            if !(what == "event" || (what == "mention" && lane == "gdelt")) {
                continue;
            }
            out.push(serde_json::json!({
                "says": d["says"], "source": d["source"],
                "read_at": d["read_at"], "lane": lane,
            }));
        }
    }
    out.sort_by(|a, b| b["read_at"].as_str().cmp(&a["read_at"].as_str()));
    out.truncate(limit);
    Ok(out)
}

async fn news() -> impl IntoResponse {
    let render = || -> anyhow::Result<String> {
        let lines = news_lines(100)?;
        let rows: String = lines
            .iter()
            .map(|l| {
                let when: String =
                    l["read_at"].as_str().unwrap_or("").chars().take(16).collect();
                format!(
                    "<li><span class=\"nm\">{}</span>\
                     <span class=\"mk\"><a href=\"{}\" rel=\"nofollow\">source</a> · {}</span></li>",
                    esc(l["says"].as_str().unwrap_or("")),
                    safe_href(l["source"].as_str().unwrap_or("#")),
                    esc(&when),
                )
            })
            .collect();
        let body = format!(
            "<h1>News</h1><p class=\"lede\">The wire, as the catalogue's crawler heard it: \
             headlines about the AI market from its last two deliveries, newest first. \
             Nothing is stored and nothing is judged — every line links to where it was \
             said, and the <a href=\"/index/crawler\">crawler's own page</a> shows the \
             reading happening.</p>\
             <div class=\"facts\"><div><b>{}</b><span>headlines in the last deliveries</span></div></div>\
             <ul class=\"rows people\">{rows}</ul>\
             <p class=\"io\"><a href=\"/index/news.json\">this page as JSON</a></p>",
            grouped(lines.len() as i64),
        );
        Ok(shell(
            "News — the AI market wire",
            "AI-market headlines as the Pass Index crawler heard them.",
            "/index/news",
            body,
            String::new(),
        ))
    };
    match render() {
        Ok(p) => html(p),
        Err(e) => html(format!("<h1>News</h1><p class=\"none\">{}</p>", esc(&e.to_string()))),
    }
}

async fn news_json() -> impl IntoResponse {
    match news_lines(100) {
        Ok(lines) => live_json(serde_json::json!({
            "href": "/index/news", "kind": "news",
            "count": lines.len(), "headlines": lines,
        })),
        Err(e) => live_json(serde_json::json!({"error": e.to_string()})),
    }
}
