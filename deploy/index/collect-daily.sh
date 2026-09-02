#!/bin/sh
# Daily Pass Index collection. Every source publishes its own prices; this
# run reads them and appends only what moved. Installed on pass-hub as
# /srv/pass-index/collect-daily.sh, driven by cron.
#
#   17 4 * * * /srv/pass-index/collect-daily.sh >> /var/log/pass-index.log 2>&1
set -u
COMPOSE=/srv/pass-index/src/deploy/index/docker-compose.yml
bad=0

# Every step used to end in "| tail -N", and a pipeline's status is the last
# command's — tail always succeeds — so `set -e` never fired and a step that
# failed printed nothing and the run carried on green. The quarantine wrote
# to a pen it had no permission to write for days that way. This keeps the
# short output and still notices.
step() {                        # step <tail-lines> <command...>
  lines=$1; shift
  if out=$(docker compose -f "$COMPOSE" exec -T pass-index "$@" 2>&1); then
    printf '%s\n' "$out" | tail -"$lines"
  else
    printf '!! %s FAILED\n' "$*"
    printf '%s\n' "$out" | tail -20
    bad=1
  fi
}
printf '\n=== %s ===\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
step 20 collect /data/index.db
# retire shelves an offering the collectors were reading but did not see. If
# the collect above failed, "did not see" means the collector stopped, not
# the seller — so retire must not run, or a week of broken collects marks
# live listings stale. Its own success is recorded here, before any later
# step can raise `bad`.
collected=0; [ "$bad" = 0 ] && collected=1
# Two public documents price the same models the crawlers do, from sellers
# who publish no list of their own: LiteLLM's price file and models.dev.
# They add no stock — seven names in ten are a model already here wearing
# another seller's barcode — but they add the sellers, which is the whole
# point. A model nobody recognises is counted and reported, never invented.
step 6 repair /data/index.db sellers --apply

# The crawler's findings, through the standard supplier door: exact dollar
# figures land as prices, quoted lines land as evidence, board rows as
# standings, and a known maker's new model goes to the pen. A supplier never
# mints identity.
step 7 repair /data/index.db supply --from /crawler/findings --pen /data/quarantine.db --apply

# The boards publish more than the catalogue used to take: 42 leaderboards
# rank 4,287 placements and we held 811 of them. This reads the ones it knows
# how to read, all the way down, and is the authority for those boards — an
# older crawl's row under a different metric label would otherwise stand
# beside the new one as a second result for one measurement.
# The gateways — a dozen companies reselling everybody's models behind one
# API and publishing the whole price list without a key — are read by the
# collect binary above, which is what lets the catalogue say a model is sold
# by twenty-nine companies rather than by one.
step 5 repair /data/index.db boards --apply

# What is offered without charge, which is a different fact from a low price
# and has to be collected as one. Only a nought the seller declared counts.
step 3 repair /data/index.db free --apply

# Free tiers, which no feed publishes: somebody read each page and wrote the
# allowance down. Re-applied nightly so a withdrawn allowance stops being
# advertised the day it is taken out of the table.
step 3 repair /data/index.db freetiers --apply

# Why the catalogue keeps a company nobody can price. A judgement, signed,
# because the significance of a company that publishes no price cannot be
# computed from a catalogue of prices.
# Plans bought by the month, and what each allows. Re-applied nightly so a
# plan whose price moved stops advertising the old one.
step 2 repair /data/index.db plans --apply

step 2 repair /data/index.db opaque --apply

# Models the market sells that the catalogue does not hold. A name earns an
# entity only when every marker of how it is served comes off and it still
# matches nothing here, and only when the feed that named it also priced it.
step 3 repair /data/index.db newmodels --apply

# What a thing is, beyond what it costs: when it came out, what it was
# trained up to, how much it will read and write back, whether it reasons and
# whether it calls tools. The same two feeds the price collectors read; each
# fact settled by the rule its own nature demands rather than one rule for six.
step 3 repair /data/index.db facts --apply

# Whether the weights are published, which models.dev states on every model
# it lists and the price collector above throws away. The sellers vote,
# because one of them marks other people's open weights closed.
step 3 repair /data/index.db weights --apply

# Which companies run on venture money. Read off each company's own
# Wikipedia article, rounds only — a valuation is not money received.
step 3 repair /data/index.db startups --apply

# Who founded each company and who runs it, from Wikidata off the article
# the round reader already found. Two facts, both sourced.
step 2 repair /data/index.db people --apply

# Davidovs Venture Collective's portfolio, from the fund's own job board, and
# everything findable about each company in it.
step 2 repair /data/index.db dvc --apply
step 2 repair /data/index.db enrich --fund "Davidovs Venture Collective" --apply

# Who put the money in. Y Combinator states its own portfolio; the rest is
# read out of the same round sentences the funding figures came from, and a
# name counts only where the sentence names it as an investor.
step 2 repair /data/index.db funds --apply

# The vocabulary. Written by hand and re-applied nightly, so a correction
# lands the same night it is made.
step 1 repair /data/index.db terms --apply

# How big a model is, where its own name says so. The catalogue held a
# parameter count for 126 models of fourteen hundred; the makers put it in
# the name of nearly three hundred more.
step 2 repair /data/index.db sizes --apply

# One spelling per brand, decided by the company that owns it. Feeds shout or
# whisper as they please — QWEN and Qwen, DeepSeek and Deepseek — and a
# catalogue that spells one company three ways cannot be searched.
step 2 repair /data/index.db naming --apply

# Rows that are the same thing written two ways, found by the resolver
# refusing to bind a name two entities both answer to.
step 2 repair /data/index.db fold --apply

# Anything a feed offered that no company we have looked at sells. It moves to
# its own database, because a catalogue with one invented row in it is worth
# less than a catalogue with a hundred missing ones.
#
# It runs AFTER the fund collectors, not before them. Its other job is to
# release a candidate that has since arrived by another road, and the funds
# arrive by exactly that road — so run earlier, it released nothing that had
# not yet come, and the nightly check then found 131 rows sitting in the pen
# and the catalogue at once and failed every night.
step 2 repair /data/index.db quarantine --pen /data/quarantine.db --apply

# An alias filed on a row further from its own text than another row's name.
# It is matched before anything is stripped, so a misfiled one beats the
# right answer every time and puts a seller's price on the wrong card.
step 2 repair /data/index.db aliases --apply

# One description per thing. A card prints one and the catalogue held up to
# five, so which a reader saw was decided by row order. The maker's own page
# wins; among third parties, the most recent reading.
step 2 repair /data/index.db descriptions --apply

# Corrections the feeds keep re-sending. Runs before the checks, because a
# contradiction that arrives again every night is not something to re-fix by
# hand each morning.
step 3 repair /data/index.db normalise --apply

# Offerings the seller has stopped listing come off the shelf. The seller was
# read, the row was not there: the price is no longer anyone's to charge.
if [ "$collected" = 1 ]; then
  step 3 repair /data/index.db retire --apply
else
  printf '!! retire skipped: collect failed, so an unseen offering means the collector stopped, not the seller\n'
fi

# Both suites record what they found, so the coverage page can show the same
# verdict that would have stopped this run rather than a claim about it.
# The quarantine depth is the operator's queue: a growing number means the
# market shipped things the catalogue has not been taught to recognise.
step 1 mint /data/index.db list
# Whatever the crawlers wrote overnight, check that the catalogue still holds
# together before anyone reads it. A blocking finding means the page is
# asserting something false; it is logged loudly and the run exits non-zero
# so cron mails it.
# Both suites run, whatever either finds, and the run fails at the end if
# anything blocked. Chained with exit, the page walk only ever ran on a night
# the data was already spotless — which is the night it is least needed. It
# ran once in five, on 2,840 pages, while the catalogue grew to 4,230.
# What each thing is for.
step 1 repair /data/index.db tasks --apply

docker compose -f "$COMPOSE" exec -T pass-index check /data/index.db 2>&1 || {
  printf "!! Pass Index consistency FAILED — see the findings above\n"
  bad=1
}

# The database can be consistent and the pages still broken: an address that
# 404s, a page that stopped saying when it was read, two pages competing for
# one title. This walks what a reader is actually handed.
docker compose -f "$COMPOSE" exec -T pass-index audit \
        --base http://127.0.0.1:8185 --db /data/index.db 2>&1 || {
  printf "!! Pass Index pages FAILED — see the findings above\n"
  bad=1
}

# Tell the engines what changed tonight. Google killed its sitemap ping;
# IndexNow (Bing and partners — the index ChatGPT Search reads) is the door
# that still opens. Only pages whose lastmod is tonight's date are sent.
KEY_FILE=/srv/pass-index/indexnow.key
if [ -f "$KEY_FILE" ]; then
  TODAY=$(date -u +%F)
  curl -s http://127.0.0.1:8185/index/sitemap.xml \
    | grep -oE "<loc>[^<]+</loc><lastmod>$TODAY" \
    | sed 's|<loc>||;s|</loc>.*||' \
    | jq -R -s --arg key "$(cat $KEY_FILE)" \
        'split("\n") | map(select(length>0)) | {host:"pass.io", key:$key,
         keyLocation:("https://pass.io/" + $key + ".txt"), urlList:.[0:9000]}' \
    | curl -s -X POST https://api.indexnow.org/indexnow \
        -H 'Content-Type: application/json; charset=utf-8' -d @- \
        -o /dev/null -w "IndexNow: %{http_code}\n"
fi

exit $bad
