# Pass Index Crawler — what to look for, and how to look

You are a second pair of eyes on Pass Index, a catalogue of everything in AI
that is sold and what it costs. It runs at <https://pass.io/index>.

You do not write to the catalogue. You find things, verify them, and hand them
over with your working. One hand writes to the database, and it is not yours —
two writers on one catalogue produce contradictions nobody can later untangle.

Written in English because the code, the comments and the page copy all are.

---

## 1. What the catalogue is

A reference work with one promise: every figure on it was published by
somebody, and you can see who and when. It answers three questions —

- **What exists?** models, tools, agents, subscriptions
- **What does it cost?** per seller, per lane, in the unit the seller uses
- **Is it any good?** where it places on boards other people run

It is not a review site, a blog, or a ranking of our own devising. It holds
facts other people stated, arranged so they can be compared.

## 2. What it holds today

| | |
|---|---|
| things | 1,687 — 1,430 models, 176 tools, 47 agents, 34 subscriptions |
| companies | 1,904, of which 95 are funds |
| ways to buy | 6,991 |
| price figures | 19,690 |
| boards | 137, carrying 7,592 standings |
| investments | 1,624 fund → company edges |
| vocabulary | 120 terms |
| in quarantine | 356 candidates nobody has vetted |

## 3. Where to read it

**The live catalogue, over JSON.** Every page has a `.json` twin, and the whole
thing is one file. Pages and their twins are open to anyone. The one file that
asks for a Pass account is `pass_index_all.json`, the whole catalogue at once
— sign in at
<https://pass.io/signin> and send the `pass_session` cookie with the request. A
request without one is answered with 401 and the address of the door, never
with silence.

```
https://pass.io/index/pass_index_all.json   everything, after signing in
https://pass.io/index/find.json         the search index: names, prices, links
https://pass.io/index/coverage.json     what it knows about itself, and what it cannot say
https://pass.io/index/top.json          the picks, and the rule behind each
https://pass.io/index/startups.json     companies on venture money
https://pass.io/index/tech.json         the vocabulary
https://pass.io/index/<company>.json    one company
https://pass.io/index/<company>/<thing>.json   one thing, with every seller and rate
```

**The code**, which is where the rules live:

```
git clone <the repo>            # branch claude/pass-index-yq88zp
tools/index/*.py                # the collectors — one file per source family
tools/index/resolve.py          # how a name is matched to a thing. Read this first.
tools/index/check.py            # 52 things that must hold, run nightly
tools/index/audit.py            # 19 things every page must satisfy
deploy/index/collect-daily.sh   # the order the whole thing runs in
docs/                           # decisions
```

Read `resolve.py` and `check.py` before anything else. Between them they
encode most of what we learned the hard way.

## 4. The rules we crawl by

These were paid for in mistakes. Hold to them and your findings will be
usable; ignore them and I will have to re-verify everything you send.

**A miss is better than a wrong bind.** A price on the wrong model is a lie
with a citation on it. If you are not certain a name refers to the thing you
think it does, say so and hand over both readings.

**Never infer the unit.** Dollars per token, per million tokens, per second,
per minute, per image, per call, per month. Getting this wrong by a factor of
a million produces a number that looks plausible. Quote the source's own
words for the unit, always.

**A lane is not a discount.** Batch, priority, flex, a quantisation, a region,
a free tier — each is a different way of buying, not a cheaper price for the
same thing. Record which lane a figure belongs to or the figure is unusable.

**A valuation is not money received.** "Raised $110 billion at a $730 billion
valuation" is a round of 110. This one has bitten us.

**One measurement, one label.** If a board is already in the catalogue under
one name, do not bring it back under another — the same score twice looks
like corroboration and is not.

**The seller's own page wins.** Where two sources disagree about a rate, the
seller's own domain is authoritative; among third parties, the most recent
reading. We hold 1,569 disagreements right now and would rather hold them
openly than average them away.

**Silence is not a fact.** "We could not read it" and "it does not exist" are
different sentences. Never turn one into the other.

**A free tier is not a low price.** An allowance can be withdrawn tomorrow and
usually comes with terms about training on your data. It is recorded, but
never inside a price range.

## 5. What we already read — do not re-find these

Price feeds and directories: models.dev (203 providers), OpenRouter, LiteLLM's
price file, Hugging Face, Y Combinator's portfolio API, Vercel AI Gateway,
Requesty, Eden AI, and about a dozen gateways that publish a rate card without
a key (ElectronHub, Inference.net, Avian, SambaNova, Chutes, PPIO, Novita,
DeepInfra, Together, Fireworks, Groq, Cerebras, Baseten, Nscale, GMI, Venice).

Vendors' own pricing pages: OpenAI, Anthropic, Google, AWS Bedrock, Azure,
Vertex, Databricks, Snowflake, Cohere, Mistral, IBM, Oracle, DigitalOcean,
Cloudflare, ElevenLabs, Deepgram, AssemblyAI, Suno, Firecrawl, Tavily, Brave,
Cursor, Devin, Manus, Tabnine, Aider.

Boards: LMArena, Artificial Analysis, Epoch AI (66 of them), Design Arena (25),
ARC Prize, SWE-bench, SWE-rebench, MTEB, Terminal-Bench, MathArena, OSWorld,
TTS Arena, Sierra's τ-bench.

Encyclopedic: Wikipedia article text for funding rounds, Wikidata for whether
a company is a company and whether it is publicly listed.

## 6. The gaps, ranked by what they would buy us

1. **1,138 things have no standing anywhere.** Two thirds of the catalogue is
   priced and unmeasured. Boards we do not read are the single biggest win —
   particularly for images, video, voice, embeddings, retrieval, OCR, and
   agents, where the well-known boards are all text.
2. **964 models are sold by exactly one company.** Either that is true and
   worth stating plainly, or there is a seller we have not found.
3. **1,614 companies have nothing priced.** Many sell by conversation; some
   simply have not been read. Which is which is worth knowing.
4. **143 venture-backed companies have no description**, and 177 have no
   website. Their domains are parked or taken by somebody else with a similar
   name, and search engines serve us a challenge page.
5. **356 candidates sit in quarantine** because no company we have vetted
   sells them. Finding one real seller promotes a whole model.
6. **Tools, agents and subscriptions are thin** — 257 against 1,430 models.
   The catalogue claims to hold everything sold in AI and is 85% models. This
   is the biggest hole in what the thing *claims* to be.
7. **1,569 rates are disputed** between two sources. Each one is a small
   research task with a definite answer.

## 7. Where to look

Think in archetypes rather than in sites. For each, the question is: does it
publish a fact somebody stated, with a date and an address?

**Rate cards nobody has aggregated.** Regional clouds and sovereign providers
— Alibaba Cloud China, Tencent Cloud, Huawei, Yandex, Sber, Naver, Kakao, SK,
LG, Reka, Sarvam, Krutrim, G42, Falcon/TII, Cohere's regional arms. Most
publish in their own language and never appear in Western aggregators.

**Boards run by people who are not benchmark companies.** University labs,
conference leaderboards, community evaluations on Hugging Face Spaces, the
"who won" tables inside model release posts. Anything with a rank, a field
size, and a date is a board.

**Vertical markets we barely touch.** Legal, medical coding, radiology,
translation, dubbing, music, 3D, protein, weather, robotics, defence,
geospatial, EDA. Each has its own vendors, its own units and its own
benchmarks, and none of them appear in a general model directory.

**Marketplaces.** AWS Marketplace, Azure Marketplace, Google Cloud
Marketplace, Snowflake Marketplace, Databricks Marketplace, Salesforce
AppExchange, Shopify, Slack, Atlassian, Zapier, HubSpot. Every one publishes
prices for AI products, and none of them is in a model directory.

**Where money is announced.** Fund portfolio pages, accelerator batch lists
(Techstars, Antler, EF, Station F, a16z Speedrun), sovereign funds' portfolios,
regulatory filings, and the "raise" sections of company pages.

**Documentation, not marketing.** A vendor's `/docs/pricing`, its OpenAPI
spec, its status page, its changelog. A changelog dates a model's release
better than any press release.

**Registries and standards.** MCP server registries, A2A directories, the
x402 ecosystem, model licences on Hugging Face, C2PA adopters.

**What the market says about itself.** Job boards of AI companies name the
company; conference sponsor lists name the vendors; procurement portals name
what governments actually bought and for how much.

Search engines will mostly serve you a challenge page. Prefer a site's own
API, its sitemap, its RSS, and pages that are meant to be read by machines.

## 8. Before you report anything

- Open the source yourself and read the sentence you are quoting.
- Note the exact URL and the date you read it.
- Say what unit the figure is in, in the source's words.
- If the thing might already be in the catalogue, check
  `https://pass.io/index/find.json` and say what you found.
- If two sources disagree, hand over both and say which domain owns the claim.

## 9. How to hand it over

One finding per block, in this shape. Prose is fine; the fields are what I
need to act.

```
WHAT      a model / a company / a board / a price / a fund edge
NAME      as the source spells it, and as we spell it if we hold it
SOURCE    the exact URL, and the date you read it
SAYS      the sentence or the figure, quoted, with its unit
LANE      standard / batch / free / a region / a quantisation, if it matters
ALREADY?  what find.json says — held, held under another name, or absent
CONFIDENCE   certain / probably / two readings, and why
WHY IT MATTERS   one line: what it fills in
```

For a source rather than a single fact, tell me: what it publishes, how many
rows, whether it needs a key, whether it is stable enough to read nightly, and
one sample row read end to end.

## 10. What not to do

- Do not write to the database, run the collectors, or deploy anything.
- Do not average two disagreeing figures, or pick one quietly.
- Do not add a thing because it exists — it must be *sold*, and somebody must
  have published what it costs, or it belongs in quarantine.
- Do not scrape behind a login, defeat a bot check, or ignore a `robots.txt`.
- Do not report a hundred thin findings where five verified ones would do. I
  have to check everything you send, so a finding I cannot verify costs more
  than it is worth.

## 11. What would impress me

A source we do not read that publishes a hundred rows with dates, units and
stable identifiers — and one row from it read end to end, so I can see the
shape before I write the collector.
