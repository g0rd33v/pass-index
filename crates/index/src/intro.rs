//! What each list is, in words, for the reader who arrived on it from a
//! search result and has no idea what they are looking at.
//!
//! These are the one editorial thing in the catalogue: every other sentence
//! it prints is a fact read off somebody's page. They are kept here rather
//! than in the database for exactly that reason — they are our writing about
//! our own taxonomy, and they should be reviewed in a diff like code.
//!
//! Each says three things: what the category is, what a buyer should notice
//! when choosing inside it, and the catch. The catch is the part worth having
//! — a list that only sells its category is an advertisement.

/// The paragraph for one value of one axis, or nothing if it has none yet.
pub fn intro(axis: &str, value: &str) -> Option<&'static str> {
    let table: &[(&str, &str)] = match axis {
        "for" => TASKS,
        "licence" => LICENCES,
        "local" => MEMORY,
        "does" => SHAPES,
        _ => return None,
    };
    table.iter().find(|(k, _)| *k == value).map(|(_, v)| *v)
}

const TASKS: &[(&str, &str)] = &[
    ("chat",
     "Models that answer in prose across whatever subject you put to them. This is \
      the largest category and the least differentiated: most of them are competent \
      at most things, so the choice usually comes down to price, context length and \
      how the maker behaves about availability. Look at what a model costs per \
      million tokens in and out — the output rate is often four or five times the \
      input rate, and it is the one that decides your bill."),
    ("reasoning",
     "Models that plan before they answer, spending extra tokens on working the \
      problem through. They win on mathematics, hard code and anything with several \
      steps, and they are measured on boards like GPQA, AIME and ARC-AGI. The catch \
      is what the thinking costs: reasoning is billed as output tokens, so the same \
      model can cost several times more per answer at a high effort than a low one, \
      and for a question that needed one turn you have paid for a monologue."),
    ("code",
     "Models and agents for reading a codebase and changing it, rather than writing \
      a snippet. The boards that matter here are the ones that run against real \
      repositories — SWE-bench, SWE-rebench, Terminal-Bench — because a model can \
      write a plausible function and still fail to make a test pass. Note whether \
      what you are looking at is a model or an agent: an agent brings the harness, \
      the file access and the loop, and is priced for it."),
    ("embedding",
     "Models that turn text into a vector so it can be searched by meaning rather \
      than by words. They are cheap — usually cents per million tokens, and often \
      priced for input only, since nothing comes back but numbers. Two things \
      decide the choice: the dimension of the vector, which sets what your database \
      will cost to hold, and whether the model was trained for your language. \
      Changing model later means re-embedding everything you have."),
    ("rerank",
     "Models that take a handful of results a search already found and put them in \
      the right order. They are the cheap second stage of retrieval: an embedding \
      search casts a wide net fast, a reranker reads the candidates properly. \
      Because they only ever see a few documents, they cost little per query, and \
      they usually improve a weak search more than a better embedding model would."),
    ("ocr",
     "Models that read documents — scans, photographs of pages, forms, tables — and \
      return text or structure. The interesting difference between them is not \
      whether they can read a clean page, which they all can, but what they do with \
      a table, a stamp, a handwritten margin or a column that breaks across pages. \
      Several are priced per page rather than per token, which makes them easy to \
      budget and hard to compare with the rest."),
    ("transcribe",
     "Models that turn recorded speech into text. They are metered by the minute or \
      the second of audio, so cost follows the length of the recording and not the \
      difficulty of it. What separates them is languages covered, whether they mark \
      who is speaking, and whether they run in real time or only on a finished \
      file — a model that is excellent on a podcast may be unusable on a live call."),
    ("speak",
     "Models that read text aloud. They are usually priced per character, which \
      makes the arithmetic simple: a thousand characters is roughly a paragraph. \
      The real choices are the voice itself, whether you may clone one, how much \
      control you have over emotion and pacing, and latency — a model that sounds \
      wonderful in a rendered file may be too slow to hold a conversation."),
    ("music",
     "Models that compose and render music from a description. They are priced per \
      generation or per second of audio rather than per token. The question that \
      decides whether you can use one commercially is not the price but the terms: \
      makers differ sharply on who owns the output and whether it may be sold, and \
      that is on the maker's page, not in the rate card."),
    ("translate",
     "Models and services aimed at moving text between languages. A general chat \
      model will translate too, and often well; a dedicated one earns its place \
      with glossaries, formality control, document formats it keeps intact, and \
      rates that assume volume. Check which direction was measured — most quality \
      figures are for translating into English, and the other direction is harder."),
    ("guard",
     "Models that classify content rather than produce it: what is unsafe, what \
      breaks a policy, what should not be shown. They sit in front of or behind a \
      bigger model, are small and cheap by design, and are the part of a system \
      nobody notices until it is wrong. The thing to check is what the model was \
      trained to catch, because policies differ and a guard tuned for one product's \
      rules will be strict in the wrong places for yours."),
    ("search",
     "Models, tools and agents that go and look something up — the open web, a \
      set of documents, a grounded answer with citations. Pricing is usually per \
      call or per result rather than per token, so the cost follows how often you \
      ask. What differs is freshness, how much of each page you get back, and \
      whether you are handed sources you can check or a summary you must trust."),
    ("crawl",
     "Tools that fetch pages and hand them back as something a model can read: \
      rendered HTML, markdown, a screenshot, a whole site map. Priced per page or \
      per session. The differences that matter are whether JavaScript is executed, \
      what happens at a login or a bot check, and how gracefully the thing fails \
      on a site that does not want to be read."),
    ("extract",
     "Tools that pull structure out of unstructured input — fields from an invoice, \
      a schema from a page, a table from a report. Some are models, some are \
      services with a model inside. Judge them on what they do when the input is \
      malformed, because that is the whole job; anything can parse a clean file."),
    ("sandbox",
     "Somewhere for a model to run the code it just wrote, isolated from anything \
      that matters. Billed by the second of compute and the memory held, not by \
      tokens, so the cost follows how long the code runs. Look at cold-start time, \
      what network access is allowed, and whether state survives between calls — an \
      agent that must reinstall its dependencies every turn is an expensive agent."),
    ("evaluate",
     "Tools for judging what a model produced: scoring runs, tracing a chain of \
      calls, catching a regression before a user does. Priced per trace or per \
      evaluation. Most use a model as the judge, which is worth knowing, because \
      it means your evaluation has a bill and an opinion of its own."),
    ("image",
     "Models that make pictures from a description, and increasingly edit the ones \
      you give them. Priced per image, sometimes by resolution, so the rate card \
      reads nothing like a language model's. Beyond the look of the output, the \
      practical differences are whether it can render legible text, whether it will \
      edit rather than regenerate, and what the licence says about selling what \
      comes out."),
    ("video",
     "Models that generate moving footage, most of them now with sound. Priced by \
      the second of output, sometimes doubled for higher resolution, which makes \
      this the most expensive thing in the catalogue to experiment with — a minute \
      of 1080p can cost more than a million tokens of text. Check the maximum clip \
      length and whether it will continue from an image you supply."),
    ("avatar",
     "Models that make a person on screen speak — a presenter, a dubbed take, a \
      talking likeness. They are sold by the minute of finished video. The binding \
      constraint is rarely quality any more; it is consent and rights, and every \
      serious vendor gates cloning a real face or voice behind a permission step."),
];

const LICENCES: &[(&str, &str)] = &[
    ("open",
     "Weights published under a licence that lets you run them where you like and \
      sell what you build — Apache 2.0, MIT and their kin impose little beyond \
      keeping the notice. This is the list to start from if you need the model on \
      your own hardware, in your own region, or simply want no third party between \
      you and it. The trade is that hosting is now your problem, and the prices \
      shown beside each one are what somebody else charges to do it for you."),
    ("open-with-conditions",
     "Weights you can download, under a licence that asks for something in return: \
      an acceptable-use policy, a naming requirement, a revenue ceiling above which \
      you must ask, a restriction on training other models. The Llama and Gemma \
      families are the familiar cases. Read the actual licence before you build on \
      one of these — the condition is usually easy to meet and occasionally fatal, \
      and which it is depends on your product rather than on the model."),
    ("noncommercial",
     "Weights published for research and explicitly not for selling. They are here \
      because they exist, are often excellent, and are worth knowing about — and \
      because the cheapest way to learn this is not after you have shipped. If a \
      model on this list is what you want, the maker will usually license it \
      commercially on request; that is a conversation, not a download."),
    ("proprietary",
     "Models whose weights are not published and which are bought through somebody's \
      API. You get the maker's infrastructure, their scale and their uptime, and no \
      way to run the thing yourself or to keep it if it is withdrawn. Most of the \
      strongest models are here, so this is less a choice than a fact about the \
      market; the choice is which seller you buy the same model from, and the \
      catalogue holds their prices side by side."),
];

const MEMORY: &[(&str, &str)] = &[
    ("8gb",
     "The memory of a phone, a base iPad or an entry-level laptop. What fits is \
      small: models of a few billion parameters, quick and cheap to run, good at \
      summarising, classifying and simple extraction, and out of their depth on \
      long reasoning. This is also where on-device makes the most sense, because \
      the alternative is a network round trip for something that takes a moment."),
    ("16gb", "The commonest laptop configuration, and the point where a genuinely \
      useful model runs locally: fourteen billion parameters at four-bit fits with \
      room to spare for the context. Expect a capable general assistant that will \
      not match the frontier, and remember that anything else the machine is doing \
      competes for the same memory."),
    ("24gb",
     "A 24 GB graphics card or a Mac configured with 24. This is the first size \
      where the well-regarded mid-weight models — the twenty-something billion \
      parameter class — run comfortably, and where a local model starts to be a \
      real alternative to an API for daily work rather than a demonstration."),
    ("32gb",
     "Enough for a thirty-billion-parameter model at four-bit with a long context, \
      or a smaller one at higher precision if quality matters more than size. A \
      practical ceiling for a laptop that also has to be a laptop."),
    ("36gb",
     "An Apple configuration, and a comfortable one: it holds what 32 GB holds \
      without the machine feeling tight, which in practice means a longer context \
      or a browser you do not have to close first."),
    ("64gb",
     "A workstation, or a well-specified Mac. Seventy-billion-parameter models fit \
      at four-bit, which is the class where local output stops being obviously \
      worse than the hosted models people pay for. Loading takes real time and the \
      machine will be warm."),
    ("96gb",
     "An Apple configuration for people who intend to run models rather than \
      occasionally try one. Comfortably holds the seventy-billion class with a very \
      long context, or a mixture-of-experts model whose active parameters are few \
      but whose weights are all resident."),
    ("128gb",
     "A large Mac or a server card. This is where the biggest openly published \
      models come within reach — and where the distinction between total and active \
      parameters starts to decide everything, because a mixture of experts computes \
      with a fraction of itself and must still be held whole."),
    ("256gb",
     "A Mac Studio at the top of its configuration, or a serious machine. Almost \
      everything openly published fits, including the large mixtures of experts. \
      At this point the constraint is no longer whether the model loads but whether \
      it generates fast enough to be worth waiting for."),
];

const SHAPES: &[(&str, &str)] = &[
    ("text-to-text",
     "Text in, text out: the plainest shape in the catalogue and the one most \
      things are. Everything from a small classifier to a frontier reasoner lives \
      here, so the shape tells you very little on its own — the task tags and the \
      price do the work."),
    ("text-to-embedding",
     "Text in, a vector out. Nothing readable comes back; the output is a list of \
      numbers meant for a database that searches by meaning. Priced for input only, \
      because there is no output to speak of."),
    ("text-to-image",
     "A description in, a picture out. Priced per image rather than per token, \
      often by resolution, and the licence on the output matters as much as the \
      quality of it."),
    ("text-to-video",
     "A description in, footage out. The most expensive shape here by a wide \
      margin: billed by the second, so a minute of output can cost more than a \
      day of text generation."),
    ("text-to-audio",
     "Text in, sound out — speech mostly, and music. Metered by the character for \
      speech and by the second for music, which makes the two hard to compare on \
      price even though they share a shape."),
    ("audio-to-text",
     "Recorded sound in, a transcript out. Billed by the length of the recording, \
      so the cost is predictable before you send it."),
    ("text-plus-image-to-text",
     "A picture and a question in, an answer in words out. This is what makes a \
      model useful on a screenshot, a chart, a scanned page or a photograph of a \
      whiteboard, and it is now the default shape for a capable general model."),
    ("text-plus-image-to-image",
     "A picture and an instruction in, an edited picture out. The shape that \
      distinguishes editing from generating: the model is meant to keep what you \
      gave it and change only what you asked about."),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_with_a_paragraph_finds_it() {
        assert!(intro("for", "reasoning").unwrap().contains("plan before"));
        assert!(intro("licence", "noncommercial").unwrap().contains("not for selling"));
        assert!(intro("local", "16gb").is_some());
    }

    #[test]
    fn a_list_without_one_says_so_rather_than_inventing() {
        assert!(intro("for", "no-such-task").is_none());
        assert!(intro("nonsense", "reasoning").is_none());
    }
}
